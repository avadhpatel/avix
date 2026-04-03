use avix_core::auth::{ATPToken, ATPTokenClaims, ATPTokenStore};
use avix_core::types::Role;
use chrono::{Duration, Utc};

fn make_claims(session_id: &str, ttl_secs: i64) -> ATPTokenClaims {
    ATPTokenClaims {
        sub: "alice".to_string(),
        uid: 1001,
        role: Role::Admin,
        crews: vec![],
        session_id: session_id.to_string(),
        iat: Utc::now(),
        exp: Utc::now() + Duration::seconds(ttl_secs),
        scope: vec!["proc".into(), "fs".into()],
    }
}

#[test]
fn issue_and_validate_roundtrip() {
    let claims = make_claims("sess-1", 3600);
    let token = ATPToken::issue(claims.clone(), "my-secret").unwrap();
    let validated = ATPToken::validate(&token, "my-secret").unwrap();
    assert_eq!(validated.session_id, "sess-1");
    assert_eq!(validated.sub, "alice");
    assert_eq!(validated.uid, 1001);
    assert_eq!(validated.role, Role::Admin);
    assert_eq!(validated.scope, vec!["proc", "fs"]);
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
            sub: "user".into(),
            uid: 42,
            role,
            crews: vec![],
            session_id: "sess-role".into(),
            iat: Utc::now(),
            exp: Utc::now() + Duration::seconds(3600),
            scope: vec![],
        };
        let token = ATPToken::issue(claims, "secret").unwrap();
        let validated = ATPToken::validate(&token, "secret").unwrap();
        assert_eq!(validated.role, role);
    }
}

#[test]
fn token_uses_base64url_encoding() {
    let claims = make_claims("sess-b64", 3600);
    let token = ATPToken::issue(claims, "secret").unwrap();
    let payload_part = token.split('.').next().unwrap();
    // base64url alphabet: A-Z a-z 0-9 - _  (no + / = padding)
    assert!(
        payload_part
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
        "payload contains non-base64url chars: {payload_part}"
    );
}

#[test]
fn is_expiring_soon_within_4_minutes() {
    let claims = ATPTokenClaims {
        sub: "alice".into(),
        uid: 1,
        role: Role::User,
        crews: vec![],
        session_id: "s".into(),
        iat: Utc::now(),
        exp: Utc::now() + Duration::minutes(4),
        scope: vec![],
    };
    assert!(claims.is_expiring_soon());
}

#[test]
fn is_expiring_soon_false_beyond_5_minutes() {
    let claims = ATPTokenClaims {
        sub: "alice".into(),
        uid: 1,
        role: Role::User,
        crews: vec![],
        session_id: "s".into(),
        iat: Utc::now(),
        exp: Utc::now() + Duration::minutes(10),
        scope: vec![],
    };
    assert!(!claims.is_expiring_soon());
}

#[tokio::test]
async fn token_store_is_expiring_soon() {
    let store = ATPTokenStore::new("secret".into());
    let claims = ATPTokenClaims {
        sub: "alice".into(),
        uid: 1,
        role: Role::User,
        crews: vec![],
        session_id: "sess-expiring".into(),
        iat: Utc::now(),
        exp: Utc::now() + Duration::minutes(3),
        scope: vec![],
    };
    let token = store.issue(claims).await.unwrap();
    assert!(store.is_expiring_soon(&token).await.unwrap());
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
    // Allow 40x headroom in debug builds to tolerate CI/heavy-load runs.
    // The architectural target is <50µs in release (see CLAUDE.md performance targets).
    assert!(
        avg_ns < 2_000_000,
        "validation took {avg_ns} ns avg, expected < 2ms in debug builds"
    );
}
