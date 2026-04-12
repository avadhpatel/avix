use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tracing::warn;

use avix_client_core::{
    atp::types::HilOutcome,
    commands::spawn_agent::spawn_agent as core_spawn_agent,
    commands::{
        get_invocation as core_get_invocation,
        get_invocation_conversation as core_get_invocation_conversation,
        get_session as core_get_session, list_agents as core_list_agents,
        list_installed as core_list_installed, list_invocations as core_list_invocations,
        list_invocations_for_session as core_list_invocations_for_session,
        list_services as core_list_services, list_sessions as core_list_sessions,
        list_tools as core_list_tools, pipe_text as core_pipe_text,
        resolve_hil as core_resolve_hil, resume_session as core_resume_session,
    },
    persistence,
    state::SharedState,
};

#[derive(Clone)]
pub struct WebState {
    pub app: SharedState,
    pub events_tx: broadcast::Sender<String>,
}

#[derive(Deserialize)]
pub struct InvokeRequest {
    pub command: String,
    #[serde(default)]
    pub args: Value,
}

type InvokeResult = Result<Json<Value>, (StatusCode, String)>;

fn bad_request(msg: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, msg.to_string())
}

fn internal(msg: impl ToString) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.to_string())
}

pub async fn invoke_handler(
    State(state): State<WebState>,
    Json(req): Json<InvokeRequest>,
) -> InvokeResult {
    let s = state.app.read().await;
    match req.command.as_str() {
        "auth_status" => Ok(Json(serde_json::json!({
            "authenticated": s.is_authenticated(),
            "identity": s.config.identity,
        }))),

        "login" => {
            let identity = req.args["identity"]
                .as_str()
                .ok_or_else(|| bad_request("missing identity"))?
                .to_string();
            let credential = req.args["credential"]
                .as_str()
                .ok_or_else(|| bad_request("missing credential"))?
                .to_string();
            let save = req.args["save"].as_bool().unwrap_or(false);
            drop(s);
            let mut s = state.app.write().await;
            s.login(&identity, &credential)
                .await
                .map_err(|e| (StatusCode::UNAUTHORIZED, format!("Login failed: {e:?}")))?;
            if save {
                s.config
                    .save()
                    .map_err(|e| internal(format!("Failed to save config: {e:?}")))?;
            }
            Ok(Json(Value::Null))
        }

        "spawn_agent" => {
            let name = req.args["name"]
                .as_str()
                .ok_or_else(|| bad_request("missing name"))?
                .to_string();
            let description = req.args["description"].as_str().unwrap_or("").to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let pid = core_spawn_agent(&dispatcher, &name, &description, &[])
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(Value::String(pid.to_string())))
        }

        "resolve_hil" => {
            let id = req.args["id"]
                .as_str()
                .ok_or_else(|| bad_request("missing id"))?
                .to_string();
            let approve = req.args["approve"]
                .as_bool()
                .ok_or_else(|| bad_request("missing approve"))?;
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            let (pid, token) = {
                let pending = s.pending_hils.read().await;
                pending
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| bad_request("HIL not found"))?
            };
            let outcome = if approve {
                HilOutcome::Approved
            } else {
                HilOutcome::Denied
            };
            core_resolve_hil(&dispatcher, pid, &id, &token, approve, None)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            s.pending_hils.write().await.remove(&id);
            s.notifications.resolve_hil(&id, outcome).await;
            Ok(Json(Value::Null))
        }

        "pipe_text" => {
            let pid = req.args["pid"]
                .as_u64()
                .ok_or_else(|| bad_request("missing pid"))?;
            let text = req.args["text"]
                .as_str()
                .ok_or_else(|| bad_request("missing text"))?
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            core_pipe_text(&dispatcher, pid, &text)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(Value::Null))
        }

        "list_agents" => {
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let agents = core_list_agents(&dispatcher)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            // Return JSON-encoded string to match Tauri command's String return type.
            let json_str = serde_json::to_string(&agents).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "get_notifications" => {
            let notifications = s.notifications.all().await;
            let json_str =
                serde_json::to_string(&notifications).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "load_layout" => {
            let layout: String =
                persistence::load_json(&persistence::layout_path()).unwrap_or_default();
            Ok(Json(Value::String(layout)))
        }

        "save_layout" => {
            // Frontend sends { config: "..." } (key is "config" not "layout_json").
            let layout_json = req.args["config"]
                .as_str()
                .or_else(|| req.args["layout_json"].as_str())
                .ok_or_else(|| bad_request("missing config"))?;
            // Validate JSON before saving.
            let _: Value = serde_json::from_str(layout_json)
                .map_err(|e| bad_request(format!("invalid JSON: {e}")))?;
            persistence::save_json(&persistence::layout_path(), &layout_json)
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(Value::Null))
        }

        "ack_notif" => {
            let id = req.args["id"]
                .as_str()
                .ok_or_else(|| bad_request("missing id"))?;
            s.notifications.mark_read(id).await;
            Ok(Json(Value::Null))
        }

        "list_installed" => {
            let username = req.args["username"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let agents = core_list_installed(&dispatcher, &username)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            let json_str = serde_json::to_string(&agents).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "list_invocations" => {
            let username = req.args["username"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let agent_name = req.args["agent_name"].as_str().map(str::to_string);
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let records = core_list_invocations(&dispatcher, &username, agent_name.as_deref())
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            let json_str = serde_json::to_string(&records).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "get_invocation" => {
            let invocation_id = req.args["invocation_id"]
                .as_str()
                .ok_or_else(|| bad_request("missing invocation_id"))?
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            match core_get_invocation(&dispatcher, &invocation_id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?
            {
                Some(record) => {
                    let json_str =
                        serde_json::to_string(&record).map_err(|e| internal(e.to_string()))?;
                    Ok(Json(Value::String(json_str)))
                }
                None => Ok(Json(Value::Null)),
            }
        }

        "list_sessions" => {
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let sessions = core_list_sessions(&dispatcher, "")
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            let json_str = serde_json::to_string(&sessions).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "get_session" => {
            let session_id = req.args["session_id"]
                .as_str()
                .ok_or_else(|| bad_request("missing session_id"))?
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            match core_get_session(&dispatcher, &session_id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?
            {
                Some(session) => {
                    let json_str =
                        serde_json::to_string(&session).map_err(|e| internal(e.to_string()))?;
                    Ok(Json(Value::String(json_str)))
                }
                None => Ok(Json(Value::Null)),
            }
        }

        "resume_session" => {
            let session_id = req.args["session_id"]
                .as_str()
                .ok_or_else(|| bad_request("missing session_id"))?
                .to_string();
            let input = req.args["input"]
                .as_str()
                .ok_or_else(|| bad_request("missing input"))?
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let result = core_resume_session(&dispatcher, &session_id, &input)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            let json_str = serde_json::to_string(&result).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "get_session_messages" => {
            let session_id = req.args["session_id"]
                .as_str()
                .ok_or_else(|| bad_request("missing session_id"))?
                .to_string();
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let invocations = core_list_invocations_for_session(&dispatcher, &session_id)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            // Spawn all conversation fetches concurrently — one task per invocation.
            let handles: Vec<_> = invocations
                .iter()
                .map(|inv| {
                    let d = dispatcher.clone();
                    let id = inv["id"].as_str().unwrap_or("").to_string();
                    tokio::spawn(async move {
                        core_get_invocation_conversation(&d, &id)
                            .await
                            .unwrap_or_default()
                    })
                })
                .collect();
            let mut result = Vec::with_capacity(invocations.len());
            for (inv, handle) in invocations.iter().zip(handles) {
                let entries = handle.await.unwrap_or_default();
                let inv_id = inv["id"].as_str().unwrap_or("");
                let agent_name = inv["agentName"]
                    .as_str()
                    .or_else(|| inv["agent_name"].as_str())
                    .unwrap_or("");
                let status = inv["status"].as_str().unwrap_or("");
                result.push(serde_json::json!({
                    "invocationId": inv_id,
                    "agentName": agent_name,
                    "status": status,
                    "entries": entries,
                }));
            }
            let json_str = serde_json::to_string(&result).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        "get_services" => {
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let services = core_list_services(&dispatcher)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(
                serde_json::to_value(services).map_err(|e| internal(e.to_string()))?,
            ))
        }

        "get_tools" => {
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let tools = core_list_tools(&dispatcher)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(
                serde_json::to_value(tools).map_err(|e| internal(e.to_string()))?,
            ))
        }

        "install_agent" => {
            let source = req.args["source"]
                .as_str()
                .ok_or_else(|| bad_request("missing source"))?
                .to_string();
            let scope = req.args["scope"].as_str().unwrap_or("user").to_string();
            let version = req.args["version"].as_str().map(str::to_string);
            let checksum = req.args["checksum"].as_str().map(str::to_string);
            let no_verify = req.args["no_verify"].as_bool().unwrap_or(false);
            let session_id = req.args["session_id"].as_str().map(str::to_string);
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            let body = serde_json::json!({
                "source": source,
                "scope": scope,
                "version": version.unwrap_or_else(|| "latest".to_string()),
                "checksum": checksum,
                "no_verify": no_verify,
                "session_id": session_id,
            });
            drop(s);
            let mut cmd =
                avix_client_core::atp::types::Cmd::new("proc", "package/install-agent", "", body);
            cmd.token = dispatcher.token.clone();
            dispatcher
                .call(&cmd)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(Value::Null))
        }

        "install_service" => {
            let source = req.args["source"]
                .as_str()
                .ok_or_else(|| bad_request("missing source"))?
                .to_string();
            let scope = req.args["scope"].as_str().unwrap_or("system").to_string();
            let version = req.args["version"].as_str().map(str::to_string);
            let checksum = req.args["checksum"].as_str().map(str::to_string);
            let no_verify = req.args["no_verify"].as_bool().unwrap_or(false);
            let session_id = req.args["session_id"].as_str().map(str::to_string);
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            let body = serde_json::json!({
                "source": source,
                "scope": scope,
                "version": version.unwrap_or_else(|| "latest".to_string()),
                "checksum": checksum,
                "no_verify": no_verify,
                "session_id": session_id,
            });
            drop(s);
            let mut cmd =
                avix_client_core::atp::types::Cmd::new("proc", "package/install-service", "", body);
            cmd.token = dispatcher.token.clone();
            dispatcher
                .call(&cmd)
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            Ok(Json(Value::Null))
        }

        "list_installed_agents" => {
            let dispatcher = s
                .dispatcher
                .as_ref()
                .ok_or_else(|| bad_request("not connected"))?
                .clone();
            drop(s);
            let agents = core_list_installed(&dispatcher, "")
                .await
                .map_err(|e| internal(format!("{e:?}")))?;
            let json_str = serde_json::to_string(&agents).map_err(|e| internal(e.to_string()))?;
            Ok(Json(Value::String(json_str)))
        }

        other => {
            warn!("Unknown invoke command: {other}");
            Err((StatusCode::NOT_FOUND, format!("unknown command: {other}")))
        }
    }
}

pub async fn events_handler(
    ws: WebSocketUpgrade,
    State(state): State<WebState>,
) -> impl IntoResponse {
    let rx = state.events_tx.subscribe();
    ws.on_upgrade(move |socket| handle_ws(socket, rx))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use avix_client_core::{config::ClientConfig, state::new_shared};

    fn make_state() -> WebState {
        let app = new_shared(ClientConfig::default());
        let (events_tx, _) = broadcast::channel(8);
        WebState { app, events_tx }
    }

    #[tokio::test]
    async fn unknown_command_returns_not_found() {
        let state = make_state();
        let req = InvokeRequest {
            command: "nonexistent".into(),
            args: Value::Null,
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn spawn_agent_requires_dispatcher() {
        let state = make_state();
        let req = InvokeRequest {
            command: "spawn_agent".into(),
            args: serde_json::json!({"name": "test", "description": "goal"}),
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_err());
        let (status, msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(msg.contains("not connected"));
    }

    #[tokio::test]
    async fn spawn_agent_missing_name_returns_bad_request() {
        let state = make_state();
        let req = InvokeRequest {
            command: "spawn_agent".into(),
            args: serde_json::json!({"description": "no name"}),
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_notifications_returns_json_string() {
        let state = make_state();
        let req = InvokeRequest {
            command: "get_notifications".into(),
            args: Value::Null,
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(value) = result.unwrap();
        // Should be a JSON-encoded string (matching Tauri return type).
        assert!(value.is_string());
        let s = value.as_str().unwrap();
        let parsed: Vec<Value> = serde_json::from_str(s).unwrap();
        assert!(parsed.is_empty());
    }

    #[tokio::test]
    async fn load_layout_returns_empty_string_when_missing() {
        let state = make_state();
        let req = InvokeRequest {
            command: "load_layout".into(),
            args: Value::Null,
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_ok());
        let Json(value) = result.unwrap();
        assert!(value.is_string());
    }

    #[tokio::test]
    async fn save_layout_validates_json() {
        let state = make_state();
        let req = InvokeRequest {
            command: "save_layout".into(),
            args: serde_json::json!({"config": "not-valid-json{{{"}),
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ack_notif_missing_id_returns_bad_request() {
        let state = make_state();
        let req = InvokeRequest {
            command: "ack_notif".into(),
            args: Value::Null,
        };
        let result = invoke_handler(State(state), Json(req)).await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
