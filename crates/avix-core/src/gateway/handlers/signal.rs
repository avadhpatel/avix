use serde_json::json;

use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;
use crate::signal::pipe_payload::SigPipePayload;

use super::{ipc_forward, unknown_op, HandlerCtx};

const VALID_SIGNALS: &[&str] = &[
    "SIGSTART",
    "SIGPAUSE",
    "SIGRESUME",
    "SIGKILL",
    "SIGSTOP",
    "SIGSAVE",
    "SIGPIPE",
    "SIGESCALATE",
];

pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "send" => handle_send(id, cmd.cmd.body, ctx).await,
        "subscribe" | "unsubscribe" | "list" => {
            ipc_forward(
                &id,
                &format!("kernel/signal/{op}"),
                cmd.cmd.body,
                ctx.ipc.as_ref(),
            )
            .await
        }
        op => unknown_op(id, op),
    }
}

async fn handle_send(id: String, body: serde_json::Value, ctx: &HandlerCtx) -> AtpReply {
    let signal = match body["signal"].as_str() {
        Some(s) => s,
        None => {
            return AtpReply::err(
                id,
                AtpError::new(AtpErrorCode::Eparse, "missing 'signal' field"),
            );
        }
    };

    if !VALID_SIGNALS.contains(&signal) {
        return AtpReply::err(
            id,
            AtpError::new(
                AtpErrorCode::Eparse,
                format!(
                    "unknown signal '{signal}'; valid: {}",
                    VALID_SIGNALS.join(", ")
                ),
            ),
        );
    }

    if signal == "SIGPIPE" {
        let payload_value = body.get("payload").cloned().unwrap_or(json!({}));
        let pipe_payload: SigPipePayload = match serde_json::from_value(payload_value) {
            Ok(p) => p,
            Err(e) => {
                return AtpReply::err(
                    id,
                    AtpError::new(
                        AtpErrorCode::Eparse,
                        format!("invalid SIGPIPE payload: {e}"),
                    ),
                );
            }
        };

        if let Err(e) = pipe_payload.validate() {
            return AtpReply::err(id, AtpError::new(AtpErrorCode::Eparse, e));
        }

        let params = match serde_json::to_value(&pipe_payload) {
            Ok(pv) => json!({
                "signal": "SIGPIPE",
                "target": body["target"],
                "payload": pv,
            }),
            Err(e) => {
                return AtpReply::err(id, AtpError::new(AtpErrorCode::Einternal, e.to_string()));
            }
        };

        return match ctx.ipc.call("kernel/signal/send", params).await {
            Ok(v) => AtpReply::ok(id, v),
            Err(e) => AtpReply::err(id, e),
        };
    }

    match ctx.ipc.call("kernel/signal/send", body).await {
        Ok(v) => AtpReply::ok(id, v),
        Err(e) => AtpReply::err(id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::types::AtpDomain;
    use crate::types::Role;
    use base64::Engine;
    use serde_json::json;

    fn make_cmd(op: &str, body: serde_json::Value) -> ValidatedCmd {
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "s-1".into(),
                token: "tok".into(),
                domain: AtpDomain::Signal,
                op: op.into(),
                body,
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-sig".into(),
        }
    }

    async fn make_ctx(method: &str, response: serde_json::Value) -> HandlerCtx {
        crate::gateway::handlers::test_helpers::make_test_ctx(method, response).await
    }

    #[tokio::test]
    async fn send_unknown_signal_returns_eparse() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd("send", json!({ "signal": "SIGFAKE", "target": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_valid_signal_translates_correctly() {
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let cmd = make_cmd("send", json!({ "signal": "SIGKILL", "target": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn send_sigpipe_with_valid_payload_succeeds() {
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGPIPE",
                "target": 42,
                "payload": { "text": "hello agent" }
            }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn send_sigpipe_with_empty_payload_returns_eparse() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd(
            "send",
            json!({ "signal": "SIGPIPE", "target": 42, "payload": {} }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_sigpipe_with_invalid_base64_returns_eparse() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGPIPE",
                "target": 42,
                "payload": {
                    "attachments": [{
                        "type": "inline",
                        "content_type": "image/png",
                        "encoding": "base64",
                        "data": "!!!notbase64!!!"
                    }]
                }
            }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_missing_signal_field_returns_eparse() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd("send", json!({ "target": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_sigpipe_with_inline_attachment_succeeds() {
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let data = base64::engine::general_purpose::STANDARD.encode(b"file content");
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGPIPE",
                "target": 42,
                "payload": {
                    "text": "see attachment",
                    "attachments": [{
                        "type": "inline",
                        "content_type": "text/plain",
                        "encoding": "base64",
                        "data": data
                    }]
                }
            }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }
}
