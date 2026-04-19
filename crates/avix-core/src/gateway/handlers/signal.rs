use serde_json::json;

use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;
use crate::signal::pipe_payload::SigPipePayload;

use super::{ipc_forward, unknown_op, HandlerCtx};
use tracing::instrument;

/// Map an AvixError to an AtpError, recognising the EUSED sentinel string.
fn avix_err_to_atp(e: crate::error::AvixError) -> AtpError {
    let msg = e.to_string();
    if msg.contains("EUSED") {
        AtpError::new(AtpErrorCode::Eused, "approval token already consumed")
    } else {
        AtpError::new(AtpErrorCode::Einternal, msg)
    }
}

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

#[instrument(skip_all)]
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

#[instrument(skip(id, body, ctx))]
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

    // SIGRESUME with approvalToken → route through HilManager::resolve
    if signal == "SIGRESUME" {
        let payload = body.get("payload").cloned().unwrap_or(json!({}));
        if let Some(approval_token) = payload.get("approvalToken").and_then(|v| v.as_str()) {
            let hil_id = match payload.get("hilId").and_then(|v| v.as_str()) {
                Some(h) => h.to_string(),
                None => {
                    return AtpReply::err(
                        id,
                        AtpError::new(
                            AtpErrorCode::Eparse,
                            "SIGRESUME with approvalToken requires 'hilId' in payload",
                        ),
                    );
                }
            };
            let decision = payload["decision"].as_str().unwrap_or("denied");

            return match &ctx.hil_manager {
                Some(mgr) => {
                    match mgr
                        .resolve(
                            &hil_id,
                            approval_token,
                            decision,
                            "", // resolved_by from signal caller — not available here
                            payload.clone(),
                        )
                        .await
                    {
                        Ok(_) => AtpReply::ok(id, json!({ "ok": true })),
                        Err(e) => AtpReply::err(id, avix_err_to_atp(e)),
                    }
                }
                None => AtpReply::err(
                    id,
                    AtpError::new(AtpErrorCode::Eunavail, "HIL manager not configured"),
                ),
            };
        }
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

        let pid_val = if body["pid"].is_number() { &body["pid"] } else { &json!(0) };
        let params = match serde_json::to_value(&pipe_payload) {
            Ok(pv) => json!({
                "signal": "SIGPIPE",
                "pid": pid_val,
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
    use std::sync::Arc;

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
        let cmd = make_cmd("send", json!({ "signal": "SIGFAKE", "pid": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_valid_signal_translates_correctly() {
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let cmd = make_cmd("send", json!({ "signal": "SIGKILL", "pid": 42 }));
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
                "pid": 42,
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
            json!({ "signal": "SIGPIPE", "pid": 42, "payload": {} }),
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
                "pid": 42,
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
        let cmd = make_cmd("send", json!({ "pid": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_sigresume_without_approval_token_forwards_to_ipc() {
        // SIGRESUME without approvalToken is a plain signal → goes to IPC
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let cmd = make_cmd("send", json!({ "signal": "SIGRESUME", "pid": 42 }));
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn send_sigresume_with_approval_token_no_hil_id_returns_eparse() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGRESUME",
                "pid": 42,
                "payload": { "approvalToken": "tok-123", "decision": "approved" }
            }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn send_sigresume_with_approval_token_no_hil_manager_returns_eunavail() {
        let ctx = make_ctx("kernel/signal/send", json!({})).await;
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGRESUME",
                "pid": 42,
                "payload": {
                    "approvalToken": "tok-123",
                    "hilId": "hil-001",
                    "decision": "approved"
                }
            }),
        );
        // ctx has hil_manager: None
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(reply.error.unwrap().code, AtpErrorCode::Eunavail);
    }

    #[tokio::test]
    async fn send_sigresume_with_valid_hil_manager_returns_ok() {
        use crate::gateway::event_bus::AtpEventBus;
        use crate::kernel::hil::{HilRequest, HilState, HilType, HilUrgency};
        use crate::kernel::{ApprovalTokenStore, HilManager};
        use crate::memfs::vfs::MemFs;
        use crate::signal::bus::SignalBus;
        use chrono::Utc;

        let approval_store = Arc::new(ApprovalTokenStore::new());
        let token: String = approval_store.create("hil-test").await;
        let bus = Arc::new(AtpEventBus::new());
        let vfs = Arc::new(MemFs::new());
        let signal_bus = Arc::new(SignalBus::new());
        let hil_mgr = HilManager::new(
            Arc::clone(&approval_store),
            Arc::clone(&bus),
            Arc::clone(&vfs),
            signal_bus,
            3600,
        );
        let req = HilRequest {
            api_version: "avix/v1".into(),
            kind: "HilRequest".into(),
            hil_id: "hil-test".into(),
            pid: crate::types::Pid::from_u64(99),
            agent_name: "agent-x".into(),
            hil_type: HilType::ToolCallApproval,
            tool: Some("fs/write".into()),
            args: None,
            reason: None,
            context: None,
            options: None,
            urgency: HilUrgency::Normal,
            approval_token: token.clone(),
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            state: HilState::Pending,
        };
        hil_mgr.open(req).await.unwrap();

        // Build ctx with hil_manager set
        let mock = Arc::new(crate::gateway::handlers::test_helpers::MockIpcRouter::new());
        let ctx = HandlerCtx {
            ipc: mock,
            token_store: Arc::new(crate::auth::atp_token::ATPTokenStore::new("s".into())),
            auth_svc: Arc::new(crate::auth::service::AuthService::new(
                crate::config::AuthConfig {
                    api_version: "v1".into(),
                    kind: "AuthConfig".into(),
                    policy: crate::config::auth::AuthPolicy {
                        session_ttl: "8h".into(),
                        require_tls: false,
                    },
                    identities: vec![],
                },
            )),
            hil_manager: Some(hil_mgr),
        };

        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGRESUME",
                "pid": 99,
                "payload": {
                    "approvalToken": token,
                    "hilId": "hil-test",
                    "decision": "approved"
                }
            }),
        );
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok, "expected ok, got {:?}", reply.error);
    }

    #[tokio::test]
    async fn send_sigresume_double_resolve_returns_eused() {
        use crate::gateway::event_bus::AtpEventBus;
        use crate::kernel::hil::{HilRequest, HilState, HilType, HilUrgency};
        use crate::kernel::{ApprovalTokenStore, HilManager};
        use crate::memfs::vfs::MemFs;
        use crate::signal::bus::SignalBus;
        use chrono::Utc;

        let approval_store = Arc::new(ApprovalTokenStore::new());
        let token: String = approval_store.create("hil-double").await;
        let bus = Arc::new(AtpEventBus::new());
        let vfs = Arc::new(MemFs::new());
        let signal_bus = Arc::new(SignalBus::new());
        let hil_mgr = HilManager::new(
            Arc::clone(&approval_store),
            Arc::clone(&bus),
            Arc::clone(&vfs),
            signal_bus,
            3600,
        );
        let req = HilRequest {
            api_version: "avix/v1".into(),
            kind: "HilRequest".into(),
            hil_id: "hil-double".into(),
            pid: crate::types::Pid::from_u64(100),
            agent_name: "agent-y".into(),
            hil_type: HilType::ToolCallApproval,
            tool: None,
            args: None,
            reason: None,
            context: None,
            options: None,
            urgency: HilUrgency::Normal,
            approval_token: token.clone(),
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            state: HilState::Pending,
        };
        hil_mgr.open(req).await.unwrap();

        let mock = Arc::new(crate::gateway::handlers::test_helpers::MockIpcRouter::new());
        let ctx = HandlerCtx {
            ipc: mock,
            token_store: Arc::new(crate::auth::atp_token::ATPTokenStore::new("s".into())),
            auth_svc: Arc::new(crate::auth::service::AuthService::new(
                crate::config::AuthConfig {
                    api_version: "v1".into(),
                    kind: "AuthConfig".into(),
                    policy: crate::config::auth::AuthPolicy {
                        session_ttl: "8h".into(),
                        require_tls: false,
                    },
                    identities: vec![],
                },
            )),
            hil_manager: Some(hil_mgr),
        };

        let make_resume_cmd = |tok: &str| {
            make_cmd(
                "send",
                json!({
                    "signal": "SIGRESUME",
                    "pid": 100,
                    "payload": {
                        "approvalToken": tok,
                        "hilId": "hil-double",
                        "decision": "approved"
                    }
                }),
            )
        };

        let reply1 = handle(make_resume_cmd(&token), &ctx).await;
        assert!(reply1.ok);

        let reply2 = handle(make_resume_cmd(&token), &ctx).await;
        assert!(!reply2.ok);
        assert_eq!(reply2.error.unwrap().code, AtpErrorCode::Eused);
    }

    #[tokio::test]
    async fn send_sigpipe_with_inline_attachment_succeeds() {
        let ctx = make_ctx("kernel/signal/send", json!({"ok": true})).await;
        let data = base64::engine::general_purpose::STANDARD.encode(b"file content");
        let cmd = make_cmd(
            "send",
            json!({
                "signal": "SIGPIPE",
                "pid": 42,
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
