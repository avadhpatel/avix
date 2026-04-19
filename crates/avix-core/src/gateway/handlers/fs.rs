use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

#[instrument(skip_all)]
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();
    let body = &cmd.cmd.body;

    // Enforce /secrets/ read ban at handler level (VFS also enforces, belt + suspenders)
    if matches!(op, "read" | "stat" | "watch") {
        if let Some(path) = body["path"].as_str() {
            if path.starts_with("/secrets/") {
                return AtpReply::err(
                    id,
                    AtpError::new(AtpErrorCode::Eperm, "reads from '/secrets/' are forbidden"),
                );
            }
        }
    }

    match op {
        "read" | "write" | "list" | "stat" | "watch" | "unwatch" => {
            ipc_forward(
                &id,
                &format!("kernel/fs/{op}"),
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
    use crate::types::Role;
    use serde_json::json;

    fn make_cmd(op: &str, body: serde_json::Value) -> ValidatedCmd {
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "f-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Fs,
                op: op.into(),
                body,
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-fs".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn read_secrets_returns_eperm() {
        let ctx = make_ctx("kernel/fs/read", json!({})).await;
        let cmd = make_cmd("read", json!({ "path": "/secrets/api_key" }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eperm);
    }

    #[tokio::test]
    async fn stat_secrets_returns_eperm() {
        let ctx = make_ctx("kernel/fs/stat", json!({})).await;
        let cmd = make_cmd("stat", json!({ "path": "/secrets/x" }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eperm);
    }

    #[tokio::test]
    async fn stat_translates_to_kernel_fs_stat() {
        let ctx = make_ctx("kernel/fs/stat", json!({ "size": 1024 })).await;
        let cmd = make_cmd("stat", json!({ "path": "/users/alice/data.yaml" }));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn watch_translates_to_kernel_fs_watch() {
        let ctx = make_ctx("kernel/fs/watch", json!({"watch_id": "w-1"})).await;
        let cmd = make_cmd("watch", json!({ "path": "/users/alice/" }));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx("", json!({})).await;
        let cmd = make_cmd("truncate", json!({}));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }
}
