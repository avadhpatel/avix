use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

#[instrument(skip_all)]
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "create" | "list" | "restore" | "delete" => {
            ipc_forward(
                &id,
                &format!("kernel/snap/{op}"),
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
                id: "sn-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Snap,
                op: op.into(),
                body: json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-snap".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn snap_create_translates_to_kernel_snap_create() {
        let ctx = make_ctx("kernel/snap/create", json!({"snapshot_id": "snap-1"})).await;
        let reply = handle(make_cmd("create"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn snap_delete_forwards_to_kernel() {
        let ctx = make_ctx("kernel/snap/delete", json!({})).await;
        let reply = handle(make_cmd("delete"), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx("", json!({})).await;
        let reply = handle(make_cmd("clone"), &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }
}
