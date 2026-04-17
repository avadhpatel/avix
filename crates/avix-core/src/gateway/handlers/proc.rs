use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};

/// Handle ATP proc domain commands by forwarding to kernel IPC.
/// Links: docs/spec/avix-terminal-protocol.md#6-2-proc-agent-lifecycle
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    tracing::debug!(op, id = %id, "handling proc command");
    match op {
        "list-installed" | "invocation-list" => {
            // Inject caller_identity as username when the client did not specify one.
            // This ensures "avix client agent catalog" (no --username) returns the
            // correct user's agents rather than requiring the caller to know their
            // own username.
            let ipc_method = format!("kernel/proc/{op}");
            tracing::info!(op, ipc_method = %ipc_method, "forwarding to kernel IPC");
            let mut body = cmd.cmd.body;
            if body["username"].as_str().unwrap_or("").is_empty() {
                body["username"] = serde_json::json!(cmd.caller_identity);
            }
            ipc_forward(&id, &ipc_method, body, ctx.ipc.as_ref()).await
        }
        "spawn" => {
            let ipc_method = "kernel/proc/spawn";
            tracing::info!(op, ipc_method, "forwarding to kernel IPC");
            let mut body = cmd.cmd.body;
            // Inject caller identity so the kernel records the correct user on the
            // invocation record. Also ensure session_id is present (empty = new session).
            if body["caller"].as_str().unwrap_or("").is_empty() {
                body["caller"] = serde_json::json!(cmd.caller_identity);
            }
            body.as_object_mut()
                .unwrap()
                .entry("session_id")
                .or_insert(serde_json::json!(""));
            // Inject the ATP connection session ID so IpcExecutorFactory can route
            // ownership-scoped events to the correct WebSocket connection.
            body["atp_session_id"] = serde_json::json!(cmd.caller_session_id);
            ipc_forward(&id, ipc_method, body, ctx.ipc.as_ref()).await
        }
        "kill"
        | "list"
        | "stat"
        | "pause"
        | "resume"
        | "wait"
        | "setcap"
        | "invocation-get"
        | "invocation-conversation"
        | "invocation-snapshot"
        | "message-create"
        | "message-get"
        | "message-list"
        | "part-create"
        | "part-get"
        | "part-list" => {
            let ipc_method = format!("kernel/proc/{op}");
            tracing::info!(op, ipc_method = %ipc_method, "forwarding to kernel IPC");
            ipc_forward(&id, &ipc_method, cmd.cmd.body, ctx.ipc.as_ref()).await
        }
        "session-list" | "session-get" | "session-resume" => {
            // Transform kebab-case to path: session-list -> kernel/proc/session/list
            let ipc_method = format!("kernel/proc/{}", op.replace('-', "/"));
            tracing::info!(op, ipc_method = %ipc_method, "forwarding session op to kernel IPC");
            let mut body = cmd.cmd.body;
            if body["username"].as_str().unwrap_or("").is_empty() {
                body["username"] = serde_json::json!(cmd.caller_identity);
            }
            ipc_forward(&id, &ipc_method, body, ctx.ipc.as_ref()).await
        }
        "package/install-agent"
        | "package/uninstall-agent"
        | "package/install-service"
        | "package/uninstall-service" => {
            // Transform: package/install-agent -> kernel/proc/package/install-agent
            let ipc_method = format!("kernel/proc/{}", op);
            tracing::info!(op, ipc_method = %ipc_method, "forwarding package op to kernel IPC");
            // Inject caller_identity into params for kernel to use
            let mut body = cmd.cmd.body;
            body["caller_identity"] = serde_json::json!(cmd.caller_identity);
            ipc_forward(&id, &ipc_method, body, ctx.ipc.as_ref()).await
        }
        op => {
            tracing::warn!(op, "unknown proc op");
            unknown_op(id, op)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::error::AtpErrorCode;
    use crate::gateway::atp::types::AtpDomain;
    use crate::gateway::handlers::test_helpers::make_test_ctx;
    use crate::types::Role;
    use serde_json::json;

    fn make_cmd(op: &str) -> ValidatedCmd {
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "p-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Proc,
                op: op.into(),
                body: json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-proc".into(),
        }
    }

    #[tokio::test]
    async fn pause_translates_to_kernel_proc_pause() {
        let ctx = make_test_ctx("kernel/proc/pause", json!({"ok": true})).await;
        let reply = handle(make_cmd("pause"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn resume_translates_to_kernel_proc_resume() {
        let ctx = make_test_ctx("kernel/proc/resume", json!({})).await;
        let reply = handle(make_cmd("resume"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn spawn_translates_to_kernel_proc_spawn() {
        let ctx = make_test_ctx("kernel/proc/spawn", json!({"pid": 42})).await;
        let reply = handle(make_cmd("spawn"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_test_ctx("kernel/proc/noop", json!({})).await;
        let reply = handle(make_cmd("bogus"), &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    use crate::gateway::IpcRouter;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// IPC router that captures the last call's params for assertion.
    struct CapturingIpcRouter {
        response: serde_json::Value,
        captured: Mutex<serde_json::Value>,
    }

    impl CapturingIpcRouter {
        fn new(response: serde_json::Value) -> Arc<Self> {
            Arc::new(Self {
                response,
                captured: Mutex::new(json!(null)),
            })
        }

        async fn last_params(&self) -> serde_json::Value {
            self.captured.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl IpcRouter for CapturingIpcRouter {
        async fn call(
            &self,
            _method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, crate::gateway::atp::error::AtpError> {
            *self.captured.lock().await = params;
            Ok(self.response.clone())
        }
    }

    async fn make_capturing_ctx(spy: Arc<CapturingIpcRouter>) -> HandlerCtx {
        // Build base ctx for auth/token_store, then swap the IPC router.
        let base = make_test_ctx("unused", json!(null)).await;
        HandlerCtx {
            ipc: spy as Arc<dyn IpcRouter>,
            token_store: base.token_store,
            auth_svc: base.auth_svc,
            hil_manager: base.hil_manager,
        }
    }

    #[tokio::test]
    async fn spawn_injects_caller_and_empty_session_id() {
        let spy = CapturingIpcRouter::new(json!({"pid": 5}));
        let ctx = make_capturing_ctx(Arc::clone(&spy)).await;
        let reply = handle(make_cmd("spawn"), &ctx).await;
        assert!(reply.ok);
        let params = spy.last_params().await;
        assert_eq!(params["caller"].as_str(), Some("alice"), "caller must be injected");
        assert_eq!(params["session_id"].as_str(), Some(""), "session_id must default to empty string");
        assert_eq!(params["atp_session_id"].as_str(), Some("sess-proc"), "atp_session_id must be injected from caller_session_id");
    }

    #[tokio::test]
    async fn spawn_preserves_explicit_session_id() {
        let spy = CapturingIpcRouter::new(json!({"pid": 6}));
        let ctx = make_capturing_ctx(Arc::clone(&spy)).await;
        let mut cmd = make_cmd("spawn");
        cmd.cmd.body = json!({ "session_id": "550e8400-e29b-41d4-a716-446655440000" });
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
        assert_eq!(
            spy.last_params().await["session_id"].as_str(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[tokio::test]
    async fn list_installed_injects_caller_identity_when_username_absent() {
        let spy = CapturingIpcRouter::new(json!([]));
        let ctx = make_capturing_ctx(Arc::clone(&spy)).await;
        // cmd body has no "username" field — gateway must inject caller_identity
        let reply = handle(make_cmd("list-installed"), &ctx).await;
        assert!(reply.ok);
        assert_eq!(spy.last_params().await["username"].as_str(), Some("alice"));
    }

    #[tokio::test]
    async fn list_installed_preserves_explicit_username() {
        let spy = CapturingIpcRouter::new(json!([]));
        let ctx = make_capturing_ctx(Arc::clone(&spy)).await;
        let mut cmd = make_cmd("list-installed");
        cmd.cmd.body = json!({ "username": "bob" });
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
        assert_eq!(spy.last_params().await["username"].as_str(), Some("bob"));
    }
}
