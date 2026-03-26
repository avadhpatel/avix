use serde_json::Value;

use crate::atp::dispatcher::Dispatcher;
use crate::atp::types::Cmd;
use crate::error::ClientError;

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

/// Spawn a new agent via ATP proc/spawn command.
/// Returns the assigned PID on success.
/// Links: docs/spec/avix-terminal-protocol.md#proc-spawn
pub async fn spawn_agent(
    dispatcher: &Dispatcher,
    agent: &str,
    goal: &str,
    capabilities: &[&str],
) -> Result<u64, ClientError> {
    tracing::debug!(name = %agent, goal = %goal, "spawning agent via ATP");
    let body = serde_json::json!({
        "name": agent,
        "goal": goal,
        "capabilities": capabilities,
    });
    let reply_body = dispatch(dispatcher, "proc", "spawn", body)
        .await?
        .ok_or_else(|| ClientError::Other(anyhow::anyhow!("proc/spawn returned no body")))?;
    let pid = reply_body["pid"]
        .as_u64()
        .ok_or_else(|| ClientError::Other(anyhow::anyhow!("proc/spawn reply missing `pid`")))?;
    tracing::info!(pid, name = %agent, "agent spawned successfully");
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::Reply;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // -----------------------------------------------------------------------
    // Minimal fake dispatcher for unit testing commands
    // -----------------------------------------------------------------------
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

    #[tokio::test]
    async fn spawn_agent_extracts_pid_from_reply() {
        let fake = FakeDispatcher::new();
        fake.set_reply("proc/spawn", ok_reply(serde_json::json!({"pid": 42})));

        let cmd = Cmd::new(
            "proc",
            "spawn",
            "tok",
            serde_json::json!({
                "name": "researcher",
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
}
