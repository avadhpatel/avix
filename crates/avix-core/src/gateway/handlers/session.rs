use serde_json::json;

use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{unknown_op, HandlerCtx};

pub async fn handle(cmd: ValidatedCmd, _ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "ready" => AtpReply::ok(id, json!("ack")),
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
                id: "session-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Session,
                op: op.into(),
                body: json!({}),
            },
            caller_identity: "test".into(),
            caller_role: Role::Admin,
            caller_session_id: "sess-session".into(),
        }
    }

    async fn make_ctx() -> HandlerCtx {
        make_test_ctx("", json!({})).await
    }

    #[tokio::test]
    async fn session_ready_returns_ack() {
        let ctx = make_ctx().await;
        let reply = handle(make_cmd("ready"), &ctx).await;
        assert!(reply.ok);
        assert_eq!(reply.body.unwrap(), "ack");
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let ctx = make_ctx().await;
        let reply = handle(make_cmd("bogus"), &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }
}
