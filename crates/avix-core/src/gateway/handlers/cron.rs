use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

#[instrument(skip_all)]
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "list" | "add" | "remove" | "pause" | "resume" => {
            ipc_forward(
                &id,
                &format!("kernel/cron/{op}"),
                cmd.cmd.body,
                ctx.ipc.as_ref(),
            )
            .await
        }
        op => unknown_op(id, op),
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
                id: "cr-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Cron,
                op: op.into(),
                body: json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-cron".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn list_translates_to_kernel_cron_list() {
        let ctx = make_ctx("kernel/cron/list", json!({"jobs": []})).await;
        let reply = handle(make_cmd("list"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn add_translates_to_kernel_cron_add() {
        let ctx = make_ctx("kernel/cron/add", json!({"job_id": "job-1"})).await;
        let reply = handle(make_cmd("add"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx("", json!({})).await;
        let reply = handle(make_cmd("run"), &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }
}
