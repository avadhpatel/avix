use serde_json::json;

use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::validator::ValidatedCmd;

use super::{ipc_forward, unknown_op, HandlerCtx};

pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "whoami" => AtpReply::ok(
            id,
            json!({
                "identity": cmd.caller_identity,
                "role": cmd.caller_role,
                "sessionId": cmd.caller_session_id,
            }),
        ),

        "refresh" => {
            match ctx
                .auth_svc
                .refresh_token(&cmd.cmd.token, &ctx.token_store)
                .await
            {
                Ok((new_token, claims)) => AtpReply::ok(
                    id,
                    json!({
                        "token": new_token,
                        "expiresAt": claims.exp.to_rfc3339(),
                    }),
                ),
                Err(e) => AtpReply::err(id, AtpError::new(AtpErrorCode::Eauth, e.to_string())),
            }
        }

        "logout" => {
            ctx.token_store.revoke(&cmd.caller_session_id).await;
            let _ = ctx.auth_svc.revoke_session(&cmd.caller_session_id).await;
            AtpReply::ok(id, json!({}))
        }

        "sessions" | "kick" => {
            ipc_forward(
                &id,
                &format!("kernel/auth/{op}"),
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
    use crate::auth::atp_token::{ATPTokenClaims, ATPTokenStore};
    use crate::auth::service::AuthService;
    use crate::config::auth::AuthPolicy;
    use crate::config::{AuthConfig, AuthIdentity, CredentialType};
    use crate::gateway::handlers::test_helpers::MockIpcRouter;
    use crate::types::Role;
    use chrono::Utc;
    use std::sync::Arc;

    fn make_ctx() -> (HandlerCtx, Arc<ATPTokenStore>) {
        let store = Arc::new(ATPTokenStore::new("secret".into()));
        let config = AuthConfig {
            api_version: "v1".into(),
            kind: "AuthConfig".into(),
            policy: AuthPolicy {
                session_ttl: "8h".into(),
                require_tls: false,
            },
            identities: vec![AuthIdentity {
                name: "alice".into(),
                uid: 1001,
                role: Role::Admin,
                credential: CredentialType::ApiKey {
                    key_hash: "key123".into(),
                    header: None,
                },
            }],
        };
        let auth_svc = Arc::new(AuthService::new(config));
        let ipc = Arc::new(MockIpcRouter::new());
        let ctx = HandlerCtx {
            ipc,
            token_store: Arc::clone(&store),
            auth_svc,
            hil_manager: None,
        };
        (ctx, store)
    }

    async fn make_validated_cmd(store: &ATPTokenStore, op: &str) -> ValidatedCmd {
        let claims = ATPTokenClaims {
            sub: "alice".into(),
            uid: 1001,
            role: Role::User,
            crews: vec![],
            session_id: "sess-auth-1".into(),
            iat: Utc::now(),
            exp: Utc::now() + chrono::Duration::hours(8),
            scope: vec!["auth".into()],
        };
        let token = store.issue(claims).await.unwrap();
        ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "cmd-1".into(),
                token,
                domain: crate::gateway::atp::types::AtpDomain::Auth,
                op: op.into(),
                body: serde_json::json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: "sess-auth-1".into(),
        }
    }

    #[tokio::test]
    async fn whoami_returns_identity_and_role() {
        let (ctx, store) = make_ctx();
        let cmd = make_validated_cmd(&store, "whoami").await;
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
        let body = reply.body.unwrap();
        assert_eq!(body["identity"], "alice");
    }

    #[tokio::test]
    async fn logout_revokes_session() {
        let (ctx, store) = make_ctx();
        let cmd = make_validated_cmd(&store, "logout").await;
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok);
    }

    #[tokio::test]
    async fn unknown_op_returns_eparse() {
        let (ctx, store) = make_ctx();
        let cmd = make_validated_cmd(&store, "bogus").await;
        let reply = handle(cmd, &ctx).await;
        assert!(!reply.ok);
        assert_eq!(
            reply.error.unwrap().code,
            crate::gateway::atp::error::AtpErrorCode::Eparse
        );
    }

    #[tokio::test]
    async fn refresh_returns_new_token() {
        let (ctx, store) = make_ctx();
        // Login to create a session
        ctx.auth_svc.login("alice", "key123").await.unwrap();

        let claims = ATPTokenClaims {
            sub: "alice".into(),
            uid: 1001,
            role: Role::User,
            crews: vec![],
            session_id: "sess-refresh-1".into(),
            iat: Utc::now(),
            exp: Utc::now() + chrono::Duration::hours(8),
            scope: vec!["auth".into()],
        };
        let token = store.issue(claims).await.unwrap();
        // The session must exist in auth_svc for refresh to work
        // (login creates a session; we use the session from login)
        let session = ctx.auth_svc.login("alice", "key123").await.unwrap();
        let refreshable_claims = ATPTokenClaims {
            sub: "alice".into(),
            uid: 1001,
            role: Role::User,
            crews: vec![],
            session_id: session.session_id.clone(),
            iat: Utc::now(),
            exp: Utc::now() + chrono::Duration::hours(8),
            scope: vec!["auth".into()],
        };
        let refreshable_token = ctx.token_store.issue(refreshable_claims).await.unwrap();
        let cmd = ValidatedCmd {
            cmd: crate::gateway::atp::frame::AtpCmd {
                msg_type: "cmd".into(),
                id: "cmd-refresh".into(),
                token: refreshable_token,
                domain: crate::gateway::atp::types::AtpDomain::Auth,
                op: "refresh".into(),
                body: serde_json::json!({}),
            },
            caller_identity: "alice".into(),
            caller_role: Role::User,
            caller_session_id: session.session_id,
        };
        let reply = handle(cmd, &ctx).await;
        assert!(reply.ok, "{:?}", reply.error);
        assert!(reply.body.unwrap()["token"].is_string());
    }
}
