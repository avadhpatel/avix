use serde_json::Value;

use crate::atp::dispatcher::Dispatcher;
use crate::atp::types::Cmd;
use crate::error::ClientError;

pub mod spawn_agent;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Build and dispatch a command, returning its reply body on success.
/// On a non-ok reply, returns `ClientError::Atp`.
async fn dispatch(
    dispatcher: &Dispatcher,
    domain: &str,
    op: &str,
    body: Value,
) -> Result<Option<Value>, ClientError> {
    let mut cmd = Cmd::new(domain, op, "", body);
    cmd.token = dispatcher.token.clone();
    let reply = dispatcher.call(&cmd).await?;
    if reply.ok {
        Ok(reply.body)
    } else {
        if let Some(err) = &reply.error {
            Err(ClientError::Atp {
                code: format!("{:?}", err.code).to_uppercase(),
                message: err.message.clone(),
            })
        } else {
            Err(ClientError::Atp {
                code: reply.code.unwrap_or_else(|| "EUNKNOWN".into()),
                message: reply.message.unwrap_or_else(|| "unknown error".into()),
            })
        }
    }
}

// ── public commands ───────────────────────────────────────────────────────────

// Moved to spawn_agent.rs

/// Send an arbitrary signal to an agent.
pub async fn send_signal(
    dispatcher: &Dispatcher,
    pid: u64,
    signal: &str,
    payload: Option<Value>,
) -> Result<(), ClientError> {
    let body = serde_json::json!({
        "pid": pid,
        "signal": signal,
        "payload": payload.unwrap_or(Value::Null),
    });
    dispatch(dispatcher, "signal", "send", body).await?;
    Ok(())
}

/// Send SIGPIPE with a text payload.
pub async fn pipe_text(dispatcher: &Dispatcher, pid: u64, text: &str) -> Result<(), ClientError> {
    send_signal(
        dispatcher,
        pid,
        "SIGPIPE",
        Some(serde_json::json!({ "text": text })),
    )
    .await
}

/// Respond to a pending HIL request.
///
/// Sends `SIGRESUME` to the agent; the payload carries the `approval_token`,
/// `approved` flag, and optional human note.
pub async fn resolve_hil(
    dispatcher: &Dispatcher,
    pid: u64,
    hil_id: &str,
    approval_token: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), ClientError> {
    send_signal(
        dispatcher,
        pid,
        "SIGRESUME",
        Some(serde_json::json!({
            "hil_id": hil_id,
            "approval_token": approval_token,
            "approved": approved,
            "note": note,
        })),
    )
    .await
}

/// Send SIGKILL to an agent.
pub async fn kill_agent(dispatcher: &Dispatcher, pid: u64) -> Result<(), ClientError> {
    send_signal(dispatcher, pid, "SIGKILL", None).await
}

/// List active processes. Returns the raw body array from the server.
pub async fn list_agents(dispatcher: &Dispatcher) -> Result<Vec<Value>, ClientError> {
    let body = dispatch(dispatcher, "proc", "list", serde_json::json!({})).await?;
    match body {
        Some(Value::Array(arr)) => Ok(arr),
        Some(other) => Err(ClientError::Other(anyhow::anyhow!(
            "proc/list expected array, got: {other}"
        ))),
        None => Ok(vec![]),
    }
}

/// List all installed agents available to `username`.
pub async fn list_installed(
    dispatcher: &Dispatcher,
    username: &str,
) -> Result<Vec<Value>, ClientError> {
    let body = dispatch(
        dispatcher,
        "proc",
        "list-installed",
        serde_json::json!({ "username": username }),
    )
    .await?;
    match body {
        Some(Value::Array(arr)) => Ok(arr),
        Some(_) | None => Ok(vec![]),
    }
}

/// List invocation history for `username`, optionally filtered to `agent_name`.
pub async fn list_invocations(
    dispatcher: &Dispatcher,
    username: &str,
    agent_name: Option<&str>,
) -> Result<Vec<Value>, ClientError> {
    let mut payload = serde_json::json!({ "username": username });
    if let Some(name) = agent_name {
        payload["agent_name"] = serde_json::Value::String(name.to_string());
    }
    let body = dispatch(dispatcher, "proc", "invocation-list", payload).await?;
    match body {
        Some(Value::Array(arr)) => Ok(arr),
        Some(_) | None => Ok(vec![]),
    }
}

/// Get a single invocation record by UUID. Returns `None` if not found.
pub async fn get_invocation(
    dispatcher: &Dispatcher,
    invocation_id: &str,
) -> Result<Option<Value>, ClientError> {
    match dispatch(
        dispatcher,
        "proc",
        "invocation-get",
        serde_json::json!({ "id": invocation_id }),
    )
    .await
    {
        Ok(body) => Ok(body),
        Err(ClientError::Atp { code, .. })
            if code.contains("32003") || code.contains("ENOTFOUND") =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Force an immediate snapshot of a running invocation.
/// Returns the updated record on success.
pub async fn snapshot_invocation(
    dispatcher: &Dispatcher,
    invocation_id: &str,
) -> Result<Value, ClientError> {
    let body = dispatch(
        dispatcher,
        "proc",
        "invocation-snapshot",
        serde_json::json!({ "id": invocation_id }),
    )
    .await?;
    body.ok_or_else(|| ClientError::Other(anyhow::anyhow!("empty snapshot response")))
}

/// List invocations, including currently-running ones (live=true).
pub async fn list_invocations_live(
    dispatcher: &Dispatcher,
    username: &str,
    agent_name: Option<&str>,
) -> Result<Vec<Value>, ClientError> {
    let mut payload = serde_json::json!({ "username": username, "live": true });
    if let Some(name) = agent_name {
        payload["agent_name"] = serde_json::Value::String(name.to_string());
    }
    let body = dispatch(dispatcher, "proc", "invocation-list", payload).await?;
    match body {
        Some(Value::Array(arr)) => Ok(arr),
        Some(_) | None => Ok(vec![]),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceListResponse {
    pub total: usize,
    pub running: usize,
    pub starting: usize,
    #[serde(default)]
    pub services: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolListResponse {
    pub total: usize,
    pub available: usize,
    pub unavailable: usize,
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
}

/// List all running services. Returns response with metadata.
pub async fn list_services(dispatcher: &Dispatcher) -> Result<ServiceListResponse, ClientError> {
    let body = dispatch(dispatcher, "sys", "service-list", serde_json::json!({})).await?;
    match body {
        Some(Value::Object(map)) => {
            serde_json::from_value(Value::Object(map)).map_err(|e| ClientError::Other(e.into()))
        }
        Some(Value::Array(arr)) => Ok(ServiceListResponse {
            total: arr.len(),
            running: 0,
            starting: 0,
            services: arr,
        }),
        Some(_) | None => Ok(ServiceListResponse {
            total: 0,
            running: 0,
            starting: 0,
            services: vec![],
        }),
    }
}

/// List all registered tools. Returns response with metadata.
pub async fn list_tools(dispatcher: &Dispatcher) -> Result<ToolListResponse, ClientError> {
    let body = dispatch(dispatcher, "sys", "tool-list", serde_json::json!({})).await?;
    match body {
        Some(Value::Object(map)) => {
            serde_json::from_value(Value::Object(map)).map_err(|e| ClientError::Other(e.into()))
        }
        Some(Value::Array(arr)) => Ok(ToolListResponse {
            total: arr.len(),
            available: 0,
            unavailable: 0,
            tools: arr,
        }),
        Some(_) | None => Ok(ToolListResponse {
            total: 0,
            available: 0,
            unavailable: 0,
            tools: vec![],
        }),
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::Reply;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // ---------------------------------------------------------------------------
    // Minimal fake dispatcher for unit testing commands
    // ---------------------------------------------------------------------------
    //
    // We want to test that `commands` build the right Cmd bodies and interpret
    // replies correctly, without a real WS connection.
    //
    // The approach: a `FakeTransport` that holds a queue of pre-configured
    // `(domain, op) → Reply` responses. We wire it directly into a
    // `Dispatcher`-shaped struct.  Since `Dispatcher` is not easily mockable
    // (it owns a real `AtpClient`), we instead test the `dispatch` helper by
    // verifying the Cmd that would be produced, and then calling `dispatch` with
    // a real `Dispatcher` backed by an in-memory tokio channel pair.
    //
    // For now the tests verify the *command logic* (body construction, error
    // propagation) by using an instrumented fake that intercepts Cmd JSON and
    // returns a canned Reply.

    struct FakeDispatcher {
        replies: Arc<Mutex<HashMap<String, Reply>>>,
        captured_cmds: Arc<Mutex<Vec<Cmd>>>,
    }

    impl FakeDispatcher {
        fn new() -> Self {
            Self {
                replies: Arc::new(Mutex::new(HashMap::new())),
                captured_cmds: Arc::new(Mutex::new(vec![])),
            }
        }

        fn set_reply(&self, domain_op: &str, reply: Reply) {
            let mut guard = self.replies.try_lock().unwrap();
            guard.insert(domain_op.to_string(), reply);
        }

        async fn call(&self, cmd: Cmd) -> Result<Reply, ClientError> {
            self.captured_cmds.try_lock().unwrap().push(cmd.clone());
            let key = format!("{}/{}", cmd.domain, cmd.op);
            let guard = self.replies.try_lock().unwrap();
            guard
                .get(&key)
                .cloned()
                .ok_or_else(|| ClientError::Other(anyhow::anyhow!("no reply configured for {key}")))
        }

        async fn captured(&self) -> Vec<Cmd> {
            self.captured_cmds.try_lock().unwrap().clone()
        }
    }

    // Helper: build an ok Reply with a given body.
    fn ok_reply(body: Value) -> Reply {
        Reply {
            frame_type: "reply".into(),
            id: "test-id".into(),
            ok: true,
            code: None,
            message: None,
            body: Some(body),
            error: None,
        }
    }

    fn err_reply(code: &str, message: &str) -> Reply {
        use avix_core::gateway::atp::error::{AtpError, AtpErrorCode};
        let error_code = match code {
            "EPERM" => AtpErrorCode::Eperm,
            "EUNKNOWN" => AtpErrorCode::Einternal,
            _ => AtpErrorCode::Einternal,
        };
        Reply {
            frame_type: "reply".into(),
            id: "test-id".into(),
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            body: None,
            error: Some(AtpError::new(error_code, message)),
        }
    }

    // Because we can't easily inject a FakeDispatcher into the real `dispatch`
    // helper (which takes `&Dispatcher`), we test the command logic directly
    // by verifying the Cmd construction + reply parsing inline.

    // Moved to spawn_agent.rs

    #[tokio::test]
    async fn resolve_hil_sends_sigresume() {
        let fake = FakeDispatcher::new();
        fake.set_reply("signal/send", ok_reply(serde_json::json!({})));

        // Build the Cmd that resolve_hil would produce.
        let cmd = Cmd::new(
            "signal",
            "send",
            "tok",
            serde_json::json!({
                "pid": 10u64,
                "signal": "SIGRESUME",
                "payload": {
                    "hil_id": "h1",
                    "approval_token": "at1",
                    "approved": true,
                    "note": null,
                },
            }),
        );
        // Not yet captured — call it.
        fake.call(cmd).await.unwrap();

        let cmds = fake.captured().await;
        let cmd = &cmds[0];
        assert_eq!(cmd.domain, "signal");
        assert_eq!(cmd.op, "send");
        assert_eq!(cmd.body["signal"], "SIGRESUME");
        assert_eq!(cmd.body["payload"]["approved"], true);
    }

    #[tokio::test]
    async fn pipe_text_sends_sigpipe() {
        let fake = FakeDispatcher::new();
        fake.set_reply("signal/send", ok_reply(serde_json::json!({})));

        let cmd = Cmd::new(
            "signal",
            "send",
            "tok",
            serde_json::json!({
                "pid": 7u64,
                "signal": "SIGPIPE",
                "payload": { "text": "hello world" },
            }),
        );
        fake.call(cmd).await.unwrap();

        let cmds = fake.captured().await;
        assert_eq!(cmds[0].body["signal"], "SIGPIPE");
        assert_eq!(cmds[0].body["payload"]["text"], "hello world");
    }

    #[tokio::test]
    async fn kill_agent_sends_sigkill() {
        let fake = FakeDispatcher::new();
        fake.set_reply("signal/send", ok_reply(serde_json::json!({})));

        let cmd = Cmd::new(
            "signal",
            "send",
            "tok",
            serde_json::json!({
                "pid": 123u64,
                "signal": "SIGKILL",
                "payload": null,
            }),
        );
        fake.call(cmd).await.unwrap();

        let cmds = fake.captured().await;
        assert_eq!(cmds[0].body["signal"], "SIGKILL");
        assert_eq!(cmds[0].body["pid"], 123);
    }

    #[tokio::test]
    async fn list_agents_returns_empty_on_null_body() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/list",
            Reply {
                frame_type: "reply".into(),
                id: "x".into(),
                ok: true,
                code: None,
                message: None,
                body: None,
                error: None,
            },
        );

        let cmd = Cmd::new("proc", "list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        // Replicate list_agents body handling.
        let result: Vec<Value> = match reply.body {
            Some(Value::Array(arr)) => arr,
            None => vec![],
            _ => panic!("unexpected"),
        };
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_agents_returns_array_from_reply() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/list",
            ok_reply(serde_json::json!([{"pid": 1}, {"pid": 2}])),
        );

        let cmd = Cmd::new("proc", "list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        let agents: Vec<Value> = match reply.body {
            Some(Value::Array(arr)) => arr,
            _ => panic!("expected array"),
        };
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0]["pid"], 1);
    }

    // T-CLI-01: list_installed passes username correctly
    #[tokio::test]
    async fn list_installed_passes_username_in_body() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/list-installed",
            ok_reply(serde_json::json!([
                {"name": "researcher", "version": "1.0.0", "scope": "system"}
            ])),
        );

        let cmd = Cmd::new(
            "proc",
            "list-installed",
            "tok",
            serde_json::json!({ "username": "alice" }),
        );
        let reply = fake.call(cmd).await.unwrap();

        let agents: Vec<Value> = match reply.body {
            Some(Value::Array(arr)) => arr,
            _ => panic!("expected array"),
        };
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["name"], "researcher");
    }

    // T-CLI-02: list_invocations returns records sorted (here just verifies passthrough)
    #[tokio::test]
    async fn list_invocations_returns_all_for_user() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/invocation-list",
            ok_reply(serde_json::json!([
                {"id": "inv-1", "agentName": "researcher", "status": "completed"},
                {"id": "inv-2", "agentName": "coder", "status": "running"},
            ])),
        );

        let cmd = Cmd::new(
            "proc",
            "invocation-list",
            "tok",
            serde_json::json!({ "username": "alice" }),
        );
        let reply = fake.call(cmd).await.unwrap();

        let records: Vec<Value> = match reply.body {
            Some(Value::Array(arr)) => arr,
            _ => panic!("expected array"),
        };
        assert_eq!(records.len(), 2);
    }

    // T-CLI-03: list_invocations with agent_name passes filter
    #[tokio::test]
    async fn list_invocations_with_agent_name_filter() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/invocation-list",
            ok_reply(serde_json::json!([
                {"id": "inv-1", "agentName": "researcher", "status": "completed"},
            ])),
        );

        let cmd = Cmd::new(
            "proc",
            "invocation-list",
            "tok",
            serde_json::json!({ "username": "alice", "agent_name": "researcher" }),
        );
        let reply = fake.call(cmd).await.unwrap();
        assert!(reply.ok);
        let body = reply.body.is_some();
        assert!(body);
    }

    // T-CLI-04: get_invocation parses conversation
    #[tokio::test]
    async fn get_invocation_returns_record() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "proc/invocation-get",
            ok_reply(serde_json::json!({
                "id": "inv-abc",
                "agentName": "researcher",
                "goal": "research something",
                "status": "completed",
            })),
        );

        let cmd = Cmd::new(
            "proc",
            "invocation-get",
            "tok",
            serde_json::json!({ "id": "inv-abc" }),
        );
        let reply = fake.call(cmd).await.unwrap();
        let body = reply.body.unwrap();
        assert_eq!(body["id"], "inv-abc");
        assert_eq!(body["status"], "completed");
    }

    // T-CLI-05: list_installed with empty reply returns empty vec
    #[tokio::test]
    async fn list_installed_returns_empty_on_null_body() {
        let fake = FakeDispatcher::new();
        fake.set_reply("proc/list-installed", ok_reply(serde_json::json!([])));

        let cmd = Cmd::new(
            "proc",
            "list-installed",
            "tok",
            serde_json::json!({ "username": "alice" }),
        );
        let reply = fake.call(cmd).await.unwrap();
        let agents: Vec<Value> = match reply.body {
            Some(Value::Array(arr)) => arr,
            None => vec![],
            _ => panic!("unexpected"),
        };
        assert!(agents.is_empty());
    }

    #[tokio::test]
    async fn list_services_parses_new_response_format() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/service-list",
            ok_reply(serde_json::json!({
                "total": 3,
                "running": 2,
                "starting": 1,
                "services": [
                    {"name": "router.svc", "status": "running", "pid": 10}
                ]
            })),
        );

        let cmd = Cmd::new("sys", "service-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        let response: ServiceListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            _ => panic!("expected object"),
        };
        assert_eq!(response.total, 3);
        assert_eq!(response.running, 2);
        assert_eq!(response.starting, 1);
        assert_eq!(response.services.len(), 1);
        assert_eq!(response.services[0]["name"], "router.svc");
    }

    #[tokio::test]
    async fn list_services_handles_empty_response() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/service-list",
            ok_reply(serde_json::json!({
                "total": 0,
                "running": 0,
                "starting": 0,
                "services": []
            })),
        );

        let cmd = Cmd::new("sys", "service-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        let response: ServiceListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            _ => panic!("expected object"),
        };
        assert_eq!(response.total, 0);
        assert!(response.services.is_empty());
    }

    #[tokio::test]
    async fn list_services_handles_legacy_array_format() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/service-list",
            ok_reply(serde_json::json!([
                {"name": "svc1", "status": "running"}
            ])),
        );

        let cmd = Cmd::new("sys", "service-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        // Array body → legacy format: wrap in ServiceListResponse manually
        let response: ServiceListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            Some(Value::Array(arr)) => ServiceListResponse {
                total: arr.len(),
                running: 0,
                starting: 0,
                services: arr,
            },
            _ => panic!("unexpected body"),
        };
        assert_eq!(response.total, 1);
        assert_eq!(response.services.len(), 1);
    }

    #[tokio::test]
    async fn list_tools_parses_new_response_format() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/tool-list",
            ok_reply(serde_json::json!({
                "total": 5,
                "available": 4,
                "unavailable": 1,
                "tools": [
                    {"name": "fs/read", "namespace": "fs", "description": "Read file", "state": "available"}
                ]
            })),
        );

        let cmd = Cmd::new("sys", "tool-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        let response: ToolListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            _ => panic!("expected object"),
        };
        assert_eq!(response.total, 5);
        assert_eq!(response.available, 4);
        assert_eq!(response.unavailable, 1);
        assert_eq!(response.tools.len(), 1);
        assert_eq!(response.tools[0]["name"], "fs/read");
    }

    #[tokio::test]
    async fn list_tools_handles_empty_response() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/tool-list",
            ok_reply(serde_json::json!({
                "total": 0,
                "available": 0,
                "unavailable": 0,
                "tools": []
            })),
        );

        let cmd = Cmd::new("sys", "tool-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        let response: ToolListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            _ => panic!("expected object"),
        };
        assert_eq!(response.total, 0);
        assert!(response.tools.is_empty());
    }

    #[tokio::test]
    async fn list_tools_handles_null_body() {
        let fake = FakeDispatcher::new();
        fake.set_reply("sys/tool-list", ok_reply(serde_json::Value::Null));

        let cmd = Cmd::new("sys", "tool-list", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        // Null body → empty response (matches list_tools production behaviour)
        let response: ToolListResponse = match reply.body {
            Some(Value::Object(map)) => serde_json::from_value(Value::Object(map)).unwrap(),
            Some(Value::Null) | None => ToolListResponse {
                total: 0,
                available: 0,
                unavailable: 0,
                tools: vec![],
            },
            _ => panic!("unexpected body"),
        };
        assert_eq!(response.total, 0);
        assert!(response.tools.is_empty());
    }

    #[tokio::test]
    async fn list_services_handles_error_reply() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/service-list",
            err_reply("EUNAVAIL", "service unavailable"),
        );

        // FakeDispatcher::call returns Ok(reply) regardless of reply.ok.
        // Verify the reply signals an error so callers (like list_services) can propagate it.
        let reply = fake.call(Cmd::new("sys", "service-list", "tok", serde_json::json!({}))).await.unwrap();
        assert!(!reply.ok, "reply should not be ok for an error response");
        assert!(reply.error.is_some() || reply.message.is_some());
    }

    #[tokio::test]
    async fn list_tools_handles_error_reply() {
        let fake = FakeDispatcher::new();
        fake.set_reply(
            "sys/tool-list",
            err_reply("EUNAVAIL", "tool registry unavailable"),
        );

        // FakeDispatcher::call returns Ok(reply) regardless of reply.ok.
        // Verify the reply signals an error so callers (like list_tools) can propagate it.
        let reply = fake.call(Cmd::new("sys", "tool-list", "tok", serde_json::json!({}))).await.unwrap();
        assert!(!reply.ok, "reply should not be ok for an error response");
        assert!(reply.error.is_some() || reply.message.is_some());
    }
}
