use avix_core::auth::AuthService;
use avix_core::config::AuthConfig;

fn test_config() -> AuthConfig {
    AuthConfig::from_str(
        r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 1h
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: api_key
      key_hash: "hmac-sha256:test-key-hash"
"#,
    )
    .unwrap()
}

#[tokio::test]
async fn login_with_valid_api_key_returns_session() {
    let svc = AuthService::new(test_config());
    let session = svc.login("alice", "valid-api-key").await.unwrap();
    assert!(!session.session_id.is_empty());
    assert_eq!(session.identity_name, "alice");
    assert_eq!(session.role.to_string(), "admin");
}

#[tokio::test]
async fn login_with_wrong_key_fails() {
    let svc = AuthService::new(test_config());
    // empty string is invalid
    assert!(svc.login("alice", "").await.is_err());
}

#[tokio::test]
async fn login_with_unknown_identity_fails() {
    let svc = AuthService::new(test_config());
    assert!(svc.login("ghost", "any-key").await.is_err());
}

#[tokio::test]
async fn valid_session_token_validates() {
    let svc = AuthService::new(test_config());
    let session = svc.login("alice", "valid-api-key").await.unwrap();
    let validated = svc.validate_session(&session.session_id).await.unwrap();
    assert_eq!(validated.identity_name, "alice");
}

#[tokio::test]
async fn invalid_session_token_fails() {
    let svc = AuthService::new(test_config());
    assert!(svc.validate_session("not-a-real-token").await.is_err());
}

#[tokio::test]
async fn revoked_session_fails_validation() {
    let svc = AuthService::new(test_config());
    let session = svc.login("alice", "valid-api-key").await.unwrap();
    svc.revoke_session(&session.session_id).await.unwrap();
    assert!(svc.validate_session(&session.session_id).await.is_err());
}

#[tokio::test]
async fn expired_session_fails_validation() {
    use std::time::Duration;
    let svc = AuthService::new_with_ttl(test_config(), Duration::from_millis(50));
    let session = svc.login("alice", "valid-api-key").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(svc.validate_session(&session.session_id).await.is_err());
}

#[tokio::test]
async fn multiple_sessions_independent() {
    let svc = AuthService::new(test_config());
    let s1 = svc.login("alice", "valid-api-key").await.unwrap();
    let s2 = svc.login("alice", "valid-api-key").await.unwrap();
    assert_ne!(s1.session_id, s2.session_id);
    svc.revoke_session(&s1.session_id).await.unwrap();
    assert!(svc.validate_session(&s2.session_id).await.is_ok());
}

#[tokio::test]
async fn active_session_count() {
    let svc = AuthService::new(test_config());
    assert_eq!(svc.active_session_count().await, 0);
    let s = svc.login("alice", "valid-api-key").await.unwrap();
    assert_eq!(svc.active_session_count().await, 1);
    svc.revoke_session(&s.session_id).await.unwrap();
    assert_eq!(svc.active_session_count().await, 0);
}
