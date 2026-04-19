use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;
use crate::types::Role;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

#[instrument(skip_all)]
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "get" => {
            // Self-scoping: user role may only fetch their own record
            if cmd.caller_role < Role::Operator {
                if let Some(requested) = cmd.cmd.body["username"].as_str() {
                    if requested != cmd.caller_identity {
                        return AtpReply::err(
                            id,
                            AtpError::new(
                                AtpErrorCode::Eperm,
                                format!(
                                    "'{}' may not fetch user '{}'",
                                    cmd.caller_identity, requested
                                ),
                            ),
                        );
                    }
                }
            }
            ipc_forward(&id, "kernel/users/get", cmd.cmd.body, ctx.ipc.as_ref()).await
        }
        "list" | "create" | "update" | "delete" | "passwd" => {
            ipc_forward(
                &id,
                &format!("kernel/users/{op}"),
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
    use crate::gateway::atp::types::AtpDomain;
    use crate::gateway::handlers::test_helpers::make_test_ctx;
    use serde_json::json;

    fn make_cmd(op: &str, role: Role, body: serde_json::Value) -> ValidatedCmd {
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "u-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Users,
                op: op.into(),
                body,
            },
            caller_identity: "alice".into(),
            caller_role: role,
            caller_session_id: "sess-users".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn get_own_user_allowed_for_user_role() {
        let ctx = make_ctx("kernel/users/get", json!({"username": "alice"})).await;
        let cmd = make_cmd("get", Role::User, json!({"username": "alice"}));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn get_other_user_blocked_for_user_role() {
        let ctx = make_ctx("kernel/users/get", json!({})).await;
        let cmd = make_cmd("get", Role::User, json!({"username": "bob"}));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(
            reply.error.unwrap().code,
            crate::gateway::atp::error::AtpErrorCode::Eperm
        );
    }

    #[tokio::test]
    async fn get_any_user_allowed_for_operator() {
        let ctx = make_ctx("kernel/users/get", json!({"username": "bob"})).await;
        let cmd = make_cmd("get", Role::Operator, json!({"username": "bob"}));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn get_without_username_field_allowed_for_user() {
        // No username field → defaults to self
        let ctx = make_ctx("kernel/users/get", json!({"username": "alice"})).await;
        let cmd = make_cmd("get", Role::User, json!({}));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx("", json!({})).await;
        let cmd = make_cmd("ban", Role::Admin, json!({}));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(
            reply.error.unwrap().code,
            crate::gateway::atp::error::AtpErrorCode::Eparse
        );
    }
}
