# Day 9 — Auth Service

> **Goal:** Build `auth.svc` — validates credentials against `auth.conf`, issues session tokens, tracks active sessions, and enforces TTL expiry. This is the trust root for all external (ATP) connections.

---

## Pre-flight: Verify Day 8

```bash
cargo test --workspace     # all Day 8 MemFS tests pass
grep -r "pub struct MemFs" crates/avix-core/src/
grep -r "pub struct VfsPath" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod auth;`

```
src/auth/
├── mod.rs
├── service.rs    ← AuthService struct
├── session.rs    ← Session store
└── validate.rs   ← credential validation logic
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/auth.rs`:

```rust
use avix_core::auth::AuthService;
use avix_core::config::AuthConfig;

fn test_config() -> AuthConfig {
    AuthConfig::from_str(r#"
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
"#).unwrap()
}

// ── Login ─────────────────────────────────────────────────────────────────────

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
    assert!(svc.login("alice", "wrong-key").await.is_err());
}

#[tokio::test]
async fn login_with_unknown_identity_fails() {
    let svc = AuthService::new(test_config());
    assert!(svc.login("ghost", "any-key").await.is_err());
}

// ── Session validation ────────────────────────────────────────────────────────

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

// ── Session TTL ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn expired_session_fails_validation() {
    use std::time::Duration;
    let svc = AuthService::new_with_ttl(test_config(), Duration::from_millis(50));
    let session = svc.login("alice", "valid-api-key").await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(svc.validate_session(&session.session_id).await.is_err());
}

// ── Multiple sessions ─────────────────────────────────────────────────────────

#[tokio::test]
async fn multiple_sessions_independent() {
    let svc = AuthService::new(test_config());
    let s1 = svc.login("alice", "valid-api-key").await.unwrap();
    let s2 = svc.login("alice", "valid-api-key").await.unwrap();

    assert_ne!(s1.session_id, s2.session_id);

    svc.revoke_session(&s1.session_id).await.unwrap();
    // s2 still valid
    assert!(svc.validate_session(&s2.session_id).await.is_ok());
}

// ── Active session count ──────────────────────────────────────────────────────

#[tokio::test]
async fn active_session_count() {
    let svc = AuthService::new(test_config());
    assert_eq!(svc.active_session_count().await, 0);
    let s = svc.login("alice", "valid-api-key").await.unwrap();
    assert_eq!(svc.active_session_count().await, 1);
    svc.revoke_session(&s.session_id).await.unwrap();
    assert_eq!(svc.active_session_count().await, 0);
}
```

---

## Step 3 — Implement

`AuthService` holds the `AuthConfig` and an `Arc<RwLock<HashMap<SessionId, SessionEntry>>>`. `login` validates the credential against the config (for now: hash comparison — full HMAC on Day 11), creates a `SessionEntry` with expiry, and returns it. `validate_session` checks existence and TTL. `revoke_session` removes the entry.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-09: AuthService — credential validation, session management, TTL expiry"
```

## Success Criteria

- [ ] 15+ auth tests pass
- [ ] Login with wrong credential fails
- [ ] Revoked session fails immediately
- [ ] Expired session fails after TTL
- [ ] Multiple sessions for same identity are independent
- [ ] Active session count is accurate
- [ ] 0 clippy warnings

---
---

