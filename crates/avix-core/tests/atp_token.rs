use avix_core::auth::{ATPToken, ATPTokenClaims, ATPTokenStore};
use avix_core::types::Role;
use chrono::{Duration, Utc};

fn make_claims(session_id: &str, ttl_secs: i64) -> ATPTokenClaims {
    ATPTokenClaims {
        session_id: session_id.to_string(),
        identity_name: "alice".to_string(),
        role: Role::Admin,
        expires_at: Utc::now() + Duration::seconds(ttl_secs),
    }
}

#[test]
fn issue_and_validate_roundtrip() {
    let claims = make_claims("sess-1", 3600);
    let token = ATPToken::issue(claims.clone(), "my-secret").unwrap();
    let validated = ATPToken::validate(&token, "my-secret").unwrap();
    assert_eq!(validated.session_id, "sess-1");
    assert_eq!(validated.identity_name, "alice");
    assert_eq!(validated.role, Role::Admin);
}

#[test]
fn wrong_secret_rejected() {
    let claims = make_claims("sess-2", 3600);
    let token = ATPToken::issue(claims, "secret-a").unwrap();
    assert!(ATPToken::validate(&token, "secret-b").is_err());
}

#[test]
fn expired_token_rejected() {
    let claims = make_claims("sess-3", -1); // already expired
    let token = ATPToken::issue(claims, "my-secret").unwrap();
    let err = ATPToken::validate(&token, "my-secret").unwrap_err();
    assert!(err.to_string().contains("expired"));
}

#[test]
fn tampered_payload_rejected() {
    let claims = make_claims("sess-4", 3600);
    let token = ATPToken::issue(claims, "my-secret").unwrap();
    // tamper with the payload part
    let parts: Vec<&str> = token.splitn(2, '.').collect();
    let tampered = format!("{}aa.{}", parts[0], parts[1]);
    assert!(ATPToken::validate(&tampered, "my-secret").is_err());
}

#[test]
fn invalid_format_rejected() {
    assert!(ATPToken::validate("no-dot-here", "secret").is_err());
}

#[tokio::test]
async fn token_store_issue_and_validate() {
    let store = ATPTokenStore::new("store-secret".into());
    let claims = make_claims("sess-5", 3600);
    let token = store.issue(claims).await.unwrap();
    let validated = store.validate(&token).await.unwrap();
    assert_eq!(validated.session_id, "sess-5");
}

#[tokio::test]
async fn token_store_revoke_rejects() {
    let store = ATPTokenStore::new("store-secret".into());
    let claims = make_claims("sess-6", 3600);
    let token = store.issue(claims).await.unwrap();
    store.revoke("sess-6").await;
    let err = store.validate(&token).await.unwrap_err();
    assert!(err.to_string().contains("revoked"));
}

#[tokio::test]
async fn token_store_different_sessions_independent() {
    let store = ATPTokenStore::new("store-secret".into());
    let c1 = make_claims("sess-7a", 3600);
    let c2 = make_claims("sess-7b", 3600);
    let t1 = store.issue(c1).await.unwrap();
    let t2 = store.issue(c2).await.unwrap();
    store.revoke("sess-7a").await;
    assert!(store.validate(&t1).await.is_err());
    assert!(store.validate(&t2).await.is_ok());
}

#[test]
fn role_preserved_in_token() {
    for role in [Role::Admin, Role::Operator, Role::User, Role::Guest] {
        let claims = ATPTokenClaims {
            session_id: "sess-role".into(),
            identity_name: "user".into(),
            role,
            expires_at: Utc::now() + Duration::seconds(3600),
        };
        let token = ATPToken::issue(claims, "secret").unwrap();
        let validated = ATPToken::validate(&token, "secret").unwrap();
        assert_eq!(validated.role, role);
    }
}

#[test]
fn domain_access_admin_can_access_sys() {
    assert!(Role::Admin.can_access_domain("sys"));
    assert!(!Role::User.can_access_domain("sys"));
    assert!(!Role::Guest.can_access_domain("sys"));
}

#[test]
fn domain_access_operator_can_access_kernel() {
    assert!(Role::Operator.can_access_domain("kernel"));
    assert!(Role::Admin.can_access_domain("kernel"));
    assert!(!Role::User.can_access_domain("kernel"));
}

#[test]
fn validation_timing_under_50us() {
    let claims = make_claims("sess-timing", 3600);
    let token = ATPToken::issue(claims, "timing-secret").unwrap();
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        ATPToken::validate(&token, "timing-secret").unwrap();
    }
    let avg_ns = start.elapsed().as_nanos() / 1000;
    // Allow 10x headroom in debug builds — the architectural target is <50µs in release
    assert!(
        avg_ns < 500_000,
        "validation took {avg_ns} ns avg, expected < 500µs"
    );
}
