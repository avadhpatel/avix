use crate::auth::atp_token::ATPTokenStore;
use crate::gateway::acl::{check_admin_port, check_domain_role, check_fs_hard_veto};
use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpCmd;
use crate::gateway::atp::types::AtpDomain;
use crate::gateway::replay::ReplayGuard;
use crate::types::Role;

#[derive(Debug)]
pub struct ValidatedCmd {
    pub cmd: AtpCmd,
    pub caller_identity: String,
    pub caller_role: Role,
    pub caller_session_id: String,
}

pub async fn validate_cmd(
    cmd: AtpCmd,
    token_store: &ATPTokenStore,
    replay_guard: &ReplayGuard,
    bound_session_id: &str,
    is_admin_port: bool,
) -> Result<ValidatedCmd, AtpError> {
    // Steps 2+3: HMAC + expiry
    let claims = token_store.validate(&cmd.token).await.map_err(|e| {
        let msg = e.to_string();
        if msg.contains("expired") {
            AtpError::new(AtpErrorCode::Eexpired, "token expired")
        } else {
            AtpError::new(AtpErrorCode::Eauth, format!("invalid token: {msg}"))
        }
    })?;

    // Step 4: session ID must match this WS connection
    if claims.session_id != bound_session_id {
        return Err(AtpError::new(AtpErrorCode::Esession, "session ID mismatch"));
    }

    // Step 5: domain × role matrix
    check_domain_role(cmd.domain, &cmd.op, claims.role)?;

    // Step 6: admin-port gate
    check_admin_port(cmd.domain, &cmd.op, is_admin_port)?;

    // Steps 7+8: fs hard vetoes
    if cmd.domain == AtpDomain::Fs {
        let path = cmd.body["path"].as_str().unwrap_or("");
        check_fs_hard_veto(path, &cmd.op)?;
    }

    // Replay protection
    replay_guard.check_and_register(&cmd.id).await?;

    Ok(ValidatedCmd {
        caller_identity: claims.sub.clone(),
        caller_role: claims.role,
        caller_session_id: claims.session_id.clone(),
        cmd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::atp_token::{ATPTokenClaims, ATPTokenStore};
    use crate::gateway::atp::frame::AtpCmd;
    use crate::gateway::atp::types::AtpDomain;
    use chrono::Utc;
    use serde_json::json;

    fn make_store() -> ATPTokenStore {
        ATPTokenStore::new("test-secret".to_string())
    }

    async fn make_token(store: &ATPTokenStore, session_id: &str, role: Role) -> String {
        let claims = ATPTokenClaims {
            sub: "alice".to_string(),
            uid: 1001,
            role,
            crews: vec![],
            session_id: session_id.to_string(),
            iat: Utc::now(),
            exp: Utc::now() + chrono::Duration::hours(8),
            scope: vec!["proc".into(), "fs".into(), "cap".into()],
        };
        store.issue(claims).await.unwrap()
    }

    async fn make_expired_token(store: &ATPTokenStore, session_id: &str) -> String {
        let claims = ATPTokenClaims {
            sub: "alice".to_string(),
            uid: 1001,
            role: Role::User,
            crews: vec![],
            session_id: session_id.to_string(),
            iat: Utc::now() - chrono::Duration::hours(10),
            exp: Utc::now() - chrono::Duration::hours(2),
            scope: vec!["proc".into()],
        };
        // Issue directly through the low-level ATPToken to bypass expiry check
        crate::auth::atp_token::ATPToken::issue(claims, "test-secret").unwrap()
    }

    fn make_cmd(token: &str, session_id: &str, domain: AtpDomain, op: &str) -> AtpCmd {
        AtpCmd {
            msg_type: "cmd".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            token: token.to_string(),
            domain,
            op: op.to_string(),
            body: json!({}),
        }
    }

    #[tokio::test]
    async fn valid_cmd_returns_ok() {
        let store = make_store();
        let token = make_token(&store, "sess-1", Role::User).await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-1", AtpDomain::Proc, "list");
        let result = validate_cmd(cmd, &store, &replay, "sess-1", false).await;
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v.caller_identity, "alice");
        assert_eq!(v.caller_role, Role::User);
    }

    #[tokio::test]
    async fn expired_token_returns_eexpired() {
        let store = make_store();
        let token = make_expired_token(&store, "sess-2").await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-2", AtpDomain::Proc, "list");
        let err = validate_cmd(cmd, &store, &replay, "sess-2", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eexpired);
    }

    #[tokio::test]
    async fn wrong_session_id_returns_esession() {
        let store = make_store();
        let token = make_token(&store, "sess-3", Role::User).await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-3", AtpDomain::Proc, "list");
        let err = validate_cmd(cmd, &store, &replay, "sess-WRONG", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Esession);
    }

    #[tokio::test]
    async fn guest_trying_to_spawn_returns_eperm() {
        let store = make_store();
        let token = make_token(&store, "sess-4", Role::Guest).await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-4", AtpDomain::Proc, "spawn");
        let err = validate_cmd(cmd, &store, &replay, "sess-4", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eperm);
    }

    #[tokio::test]
    async fn duplicate_command_id_returns_eparse() {
        let store = make_store();
        let token = make_token(&store, "sess-5", Role::User).await;
        let replay = ReplayGuard::new();
        let id = "dup-id-123".to_string();
        let cmd1 = AtpCmd {
            msg_type: "cmd".to_string(),
            id: id.clone(),
            token: token.clone(),
            domain: AtpDomain::Proc,
            op: "list".to_string(),
            body: json!({}),
        };
        let cmd2 = AtpCmd {
            msg_type: "cmd".to_string(),
            id: id.clone(),
            token: token.clone(),
            domain: AtpDomain::Proc,
            op: "list".to_string(),
            body: json!({}),
        };
        validate_cmd(cmd1, &store, &replay, "sess-5", false)
            .await
            .unwrap();
        let err = validate_cmd(cmd2, &store, &replay, "sess-5", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn fs_write_to_secrets_returns_eperm() {
        let store = make_store();
        let token = make_token(&store, "sess-6", Role::User).await;
        let replay = ReplayGuard::new();
        let cmd = AtpCmd {
            msg_type: "cmd".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            token: token.clone(),
            domain: AtpDomain::Fs,
            op: "write".to_string(),
            body: json!({ "path": "/secrets/api_key" }),
        };
        let err = validate_cmd(cmd, &store, &replay, "sess-6", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eperm);
    }

    #[tokio::test]
    async fn cap_grant_on_user_port_returns_eperm() {
        let store = make_store();
        let token = make_token(&store, "sess-7", Role::Admin).await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-7", AtpDomain::Cap, "grant");
        // is_admin_port = false → should fail
        let err = validate_cmd(cmd, &store, &replay, "sess-7", false)
            .await
            .unwrap_err();
        assert_eq!(err.code, AtpErrorCode::Eperm);
    }

    #[tokio::test]
    async fn cap_grant_on_admin_port_succeeds() {
        let store = make_store();
        let token = make_token(&store, "sess-8", Role::Admin).await;
        let replay = ReplayGuard::new();
        let cmd = make_cmd(&token, "sess-8", AtpDomain::Cap, "grant");
        // is_admin_port = true → should pass validation (then get EUNAVAIL stub)
        let result = validate_cmd(cmd, &store, &replay, "sess-8", true).await;
        assert!(result.is_ok());
    }
}
