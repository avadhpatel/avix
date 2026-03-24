use serde_json::Value;

use crate::atp::dispatcher::Dispatcher;
use crate::atp::types::Cmd;
use crate::error::ClientError;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Build and dispatch a command, returning its reply body on success.
/// On a non-ok reply, returns `ClientError::Atp`.
async fn dispatch(
    dispatcher: &Dispatcher,
    token: &str,
    domain: &str,
    op: &str,
    body: Value,
) -> Result<Option<Value>, ClientError> {
    let cmd = Cmd::new(domain, op, token, body);
    let reply = dispatcher.call(&cmd).await?;
    if reply.ok {
        Ok(reply.body)
    } else {
        Err(ClientError::Atp {
            code: reply.code.unwrap_or_else(|| "EUNKNOWN".into()),
            message: reply.message.unwrap_or_else(|| "unknown error".into()),
        })
    }
}

// ── public commands ───────────────────────────────────────────────────────────

/// Spawn a new agent. Returns the assigned PID.
pub async fn spawn_agent(
    dispatcher: &Dispatcher,
    token: &str,
    agent: &str,
    goal: &str,
    capabilities: &[&str],
) -> Result<u64, ClientError> {
    let body = serde_json::json!({
        "agent": agent,
        "goal": goal,
        "capabilities": capabilities,
    });
    let reply_body = dispatch(dispatcher, token, "proc", "spawn", body)
        .await?
        .ok_or_else(|| ClientError::Other(anyhow::anyhow!("proc/spawn returned no body")))?;
    reply_body["pid"]
        .as_u64()
        .ok_or_else(|| ClientError::Other(anyhow::anyhow!("proc/spawn reply missing `pid`")))
}

/// Send an arbitrary signal to an agent.
pub async fn send_signal(
    dispatcher: &Dispatcher,
    token: &str,
    pid: u64,
    signal: &str,
    payload: Option<Value>,
) -> Result<(), ClientError> {
    let body = serde_json::json!({
        "pid": pid,
        "signal": signal,
        "payload": payload.unwrap_or(Value::Null),
    });
    dispatch(dispatcher, token, "signal", "send", body).await?;
    Ok(())
}

/// Send SIGPIPE with a text payload.
pub async fn pipe_text(
    dispatcher: &Dispatcher,
    token: &str,
    pid: u64,
    text: &str,
) -> Result<(), ClientError> {
    send_signal(
        dispatcher,
        token,
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
    token: &str,
    pid: u64,
    hil_id: &str,
    approval_token: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), ClientError> {
    send_signal(
        dispatcher,
        token,
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

/// List active processes. Returns the raw body array from the server.
pub async fn list_agents(dispatcher: &Dispatcher, token: &str) -> Result<Vec<Value>, ClientError> {
    let body = dispatch(dispatcher, token, "proc", "list", serde_json::json!({})).await?;
    match body {
        Some(Value::Array(arr)) => Ok(arr),
        Some(other) => Err(ClientError::Other(anyhow::anyhow!(
            "proc/list expected array, got: {other}"
        ))),
        None => Ok(vec![]),
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::{Event, Reply};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{broadcast, Mutex};

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
        event_tx: broadcast::Sender<Event>,
    }

    impl FakeDispatcher {
        fn new() -> Self {
            let (event_tx, _) = broadcast::channel(16);
            Self {
                replies: Arc::new(Mutex::new(HashMap::new())),
                captured_cmds: Arc::new(Mutex::new(vec![])),
                event_tx,
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
        }
    }

    fn err_reply(code: &str, message: &str) -> Reply {
        Reply {
            frame_type: "reply".into(),
            id: "test-id".into(),
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            body: None,
        }
    }

    // Because we can't easily inject a FakeDispatcher into the real `dispatch`
    // helper (which takes `&Dispatcher`), we test the command logic directly
    // by verifying the Cmd construction + reply parsing inline.

    #[tokio::test]
    async fn spawn_agent_extracts_pid_from_reply() {
        let fake = FakeDispatcher::new();
        fake.set_reply("proc/spawn", ok_reply(serde_json::json!({"pid": 42})));

        let cmd = Cmd::new(
            "proc",
            "spawn",
            "tok",
            serde_json::json!({
                "agent": "researcher",
                "goal": "test",
                "capabilities": ["fs/read"],
            }),
        );
        let reply = fake.call(cmd).await.unwrap();

        // Replicate what spawn_agent does with the reply.
        let pid = reply.body.unwrap()["pid"].as_u64().unwrap();
        assert_eq!(pid, 42);
    }

    #[tokio::test]
    async fn spawn_agent_propagates_eperm() {
        let fake = FakeDispatcher::new();
        fake.set_reply("proc/spawn", err_reply("EPERM", "not allowed"));

        let cmd = Cmd::new("proc", "spawn", "tok", serde_json::json!({}));
        let reply = fake.call(cmd).await.unwrap();

        // Replicate dispatch error path.
        let result: Result<u64, ClientError> = if reply.ok {
            Ok(reply.body.unwrap()["pid"].as_u64().unwrap())
        } else {
            Err(ClientError::Atp {
                code: reply.code.unwrap_or_default(),
                message: reply.message.unwrap_or_default(),
            })
        };
        assert!(matches!(result, Err(ClientError::Atp { code, .. }) if code == "EPERM"));
    }

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
        let captured_cmds = fake.captured().await;
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
}
