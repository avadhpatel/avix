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
        "spawn" | "kill" | "list" | "stat" | "pause" | "resume" | "wait" | "setcap" => {
            let ipc_method = format!("kernel/proc/{op}");
            tracing::info!(op, ipc_method = %ipc_method, "forwarding to kernel IPC");
            let span = if op == "spawn" {
                Some(tracing::trace_span!("atp.proc.spawn"))
            } else {
                None
            };
            let _enter = span.as_ref().map(|s| s.enter());
            drop(_enter); // drop before await
            ipc_forward(&id, &ipc_method, cmd.cmd.body, ctx.ipc.as_ref()).await
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
}
