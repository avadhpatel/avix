use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

#[instrument(skip_all)]
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "inspect" | "grant" | "revoke" => {
            ipc_forward(
                &id,
                &format!("kernel/cap/{op}"),
                cmd.cmd.body,
                ctx.ipc.as_ref(),
            )
            .await
        }
        "policy/get" => {
            ipc_forward(&id, "kernel/cap/policy_get", cmd.cmd.body, ctx.ipc.as_ref()).await
        }
        "policy/set" => {
            ipc_forward(&id, "kernel/cap/policy_set", cmd.cmd.body, ctx.ipc.as_ref()).await
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

    fn make_cmd(op: &str, role: Role) -> ValidatedCmd {
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "cap-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Cap,
                op: op.into(),
                body: json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: role,
            caller_session_id: "sess-cap".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn cap_inspect_allowed_for_operator() {
        let ctx = make_ctx("kernel/cap/inspect", json!({"caps": []})).await;
        let reply = handle(make_cmd("inspect", Role::Operator), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn cap_grant_translates_to_kernel_cap_grant() {
        let ctx = make_ctx("kernel/cap/grant", json!({})).await;
        let reply = handle(make_cmd("grant", Role::Admin), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn policy_get_maps_to_correct_ipc_method() {
        let ctx = make_ctx("kernel/cap/policy_get", json!({"policy": {}})).await;
        let reply = handle(make_cmd("policy/get", Role::Admin), &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx("", json!({})).await;
        let reply = handle(make_cmd("audit", Role::Admin), &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }
}
