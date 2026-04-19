use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::instrument;

use avix_client_core::atp::types::HilOutcome;
use avix_client_core::commands::spawn_agent::spawn_agent as core_spawn_agent;
use avix_client_core::commands::{
    get_invocation as core_get_invocation,
    get_invocation_conversation as core_get_invocation_conversation,
    get_session as core_get_session, list_agents as core_list_agents,
    list_installed as core_list_installed, list_invocations as core_list_invocations,
    list_invocations_for_session as core_list_invocations_for_session,
    list_services as core_list_services, list_tools as core_list_tools,
    pipe_text as core_pipe_text, resolve_hil as core_resolve_hil,
    resume_session as core_resume_session,
};
use avix_client_core::state::SharedState;

#[derive(Serialize, Deserialize, Debug)]
pub struct SpawnAgentRequest {
    pub name: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstallRequest {
    pub source: String,
    pub scope: String,
    pub version: Option<String>,
    pub checksum: Option<String>,
    pub no_verify: bool,
    pub session_id: Option<String>,
}

#[instrument]
#[tauri::command]
pub async fn spawn_agent(
    state: State<'_, SharedState>,
    request: SpawnAgentRequest,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_spawn_agent(dispatcher, &request.name, &request.description, &[]).await {
                Ok(pid) => Ok(pid.to_string()),
                Err(e) => Err(format!("Failed to spawn agent: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn resolve_hil(
    state: State<'_, SharedState>,
    id: String,
    approve: bool,
) -> Result<(), String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            let (pid, token) = {
                let pending = s.pending_hils.read().await;
                pending.get(&id).cloned()
            }
            .ok_or("HIL not found".to_string())?;
            let outcome = if approve {
                HilOutcome::Approved
            } else {
                HilOutcome::Denied
            };
            match core_resolve_hil(dispatcher, pid, &id, &token, approve, None).await {
                Ok(_) => {
                    // Remove from pending
                    let mut pending = s.pending_hils.write().await;
                    pending.remove(&id);
                    // Update notification
                    s.notifications.resolve_hil(&id, outcome).await;
                    Ok(())
                }
                Err(e) => Err(format!("Failed to resolve HIL: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn pipe_text(
    state: State<'_, SharedState>,
    pid: u64,
    text: String,
) -> Result<(), String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            core_pipe_text(dispatcher, pid, &text)
                .await
                .map_err(|e| format!("Failed to pipe text: {:?}", e))
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn list_agents(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_agents(dispatcher).await {
                Ok(agents) => serde_json::to_string(&agents).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list agents: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn list_installed(
    state: State<'_, SharedState>,
    username: String,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_installed(dispatcher, &username).await {
                Ok(agents) => serde_json::to_string(&agents).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list installed: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn list_invocations(
    state: State<'_, SharedState>,
    username: String,
    agent_name: Option<String>,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_invocations(dispatcher, &username, agent_name.as_deref()).await {
                Ok(records) => serde_json::to_string(&records).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list invocations: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_invocation(
    state: State<'_, SharedState>,
    invocation_id: String,
) -> Result<Option<String>, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_get_invocation(dispatcher, &invocation_id).await {
                Ok(Some(record)) => serde_json::to_string(&record)
                    .map(Some)
                    .map_err(|e| e.to_string()),
                Ok(None) => Ok(None),
                Err(e) => Err(format!("Failed to get invocation: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_services(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_services(dispatcher).await {
                Ok(services) => serde_json::to_string(&services).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list services: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_tools(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_tools(dispatcher).await {
                Ok(tools) => serde_json::to_string(&tools).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list tools: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_notifications(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    let notifications = s.notifications.all().await;
    serde_json::to_string(&notifications).map_err(|e| e.to_string())
}

#[instrument]
#[tauri::command]
pub async fn auth_status(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    serde_json::to_string(&serde_json::json!({
        "authenticated": s.is_authenticated(),
        "identity": s.config.identity,
    }))
    .map_err(|e| e.to_string())
}

#[instrument]
#[tauri::command]
pub async fn login(
    state: State<'_, SharedState>,
    identity: String,
    credential: String,
    save: bool,
) -> Result<(), String> {
    let mut s = state.write().await;
    s.login(&identity, &credential)
        .await
        .map_err(|e| format!("Login failed: {e:?}"))?;
    if save {
        s.config
            .save()
            .map_err(|e| format!("Failed to save config: {e:?}"))?;
    }
    Ok(())
}

#[instrument]
#[tauri::command]
pub async fn save_layout(state: State<'_, SharedState>, layout_json: String) -> Result<(), String> {
    let _s = state.read().await;
    // Parse and save
    let _: serde_json::Value =
        serde_json::from_str(&layout_json).map_err(|e| format!("Invalid JSON: {:?}", e))?;
    avix_client_core::persistence::save_json(
        &avix_client_core::persistence::layout_path(),
        &layout_json,
    )
    .map_err(|e| format!("Failed to save layout: {:?}", e))
}

#[instrument]
#[tauri::command]
pub async fn install_agent(
    state: State<'_, SharedState>,
    request: InstallRequest,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            let body = serde_json::json!({
                "source": request.source,
                "scope": request.scope,
                "version": request.version.unwrap_or_else(|| "latest".to_string()),
                "checksum": request.checksum,
                "no_verify": request.no_verify,
                "session_id": request.session_id,
            });
            let mut cmd =
                avix_client_core::atp::types::Cmd::new("proc", "package/install-agent", "", body);
            cmd.token = dispatcher.token.clone();
            dispatcher
                .call(&cmd)
                .await
                .map_err(|e| format!("Failed to install agent: {:?}", e))?;
            Ok("OK".to_string())
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn install_service(
    state: State<'_, SharedState>,
    request: InstallRequest,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            let body = serde_json::json!({
                "source": request.source,
                "scope": request.scope,
                "version": request.version.unwrap_or_else(|| "latest".to_string()),
                "checksum": request.checksum,
                "no_verify": request.no_verify,
                "session_id": request.session_id,
            });
            let mut cmd =
                avix_client_core::atp::types::Cmd::new("proc", "package/install-service", "", body);
            cmd.token = dispatcher.token.clone();
            dispatcher
                .call(&cmd)
                .await
                .map_err(|e| format!("Failed to install service: {:?}", e))?;
            Ok("OK".to_string())
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn list_installed_agents(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            core_list_installed(dispatcher, "default")
                .await
                .map_err(|e| format!("Failed to list installed agents: {:?}", e))
                .and_then(|agents| serde_json::to_string(&agents).map_err(|e| e.to_string()))
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_session(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<Option<String>, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_get_session(dispatcher, &session_id).await {
                Ok(Some(session)) => serde_json::to_string(&session)
                    .map(Some)
                    .map_err(|e| e.to_string()),
                Ok(None) => Ok(None),
                Err(e) => Err(format!("Failed to get session: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn get_session_messages(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            let dispatcher = dispatcher.clone();
            drop(s);
            let invocations = core_list_invocations_for_session(&dispatcher, &session_id)
                .await
                .map_err(|e| format!("Failed to list invocations: {:?}", e))?;
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
                let entries = handle.await.map_err(|e| format!("Join error: {:?}", e))?;
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
            serde_json::to_value(&result).map_err(|e| e.to_string())
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[instrument]
#[tauri::command]
pub async fn resume_session(
    state: State<'_, SharedState>,
    session_id: String,
    input: String,
) -> Result<(), String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            core_resume_session(dispatcher, &session_id, &input)
                .await
                .map(|_| ())
                .map_err(|e| format!("Failed to resume session: {:?}", e))
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}
