# Day 11 — ATP Token + `avix config init`

> **Goal:** Full `ATPToken` implementation with HMAC-SHA256 signing, per-message revocation, domain access enforcement, and the `avix config init` CLI command that produces a valid `auth.conf` from scratch.

---

## Pre-flight: Verify Day 10

```bash
cargo test --workspace     # all tests pass (cumulative)
grep -r "ConcurrencyLimiter" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Extend `src/auth/` to add token issuance. Add `src/cli/` module for `avix config init`.

```
src/auth/
├── ...existing...
└── atp_token.rs    ← ATPToken struct, sign, validate, revoke

src/cli/
├── mod.rs
└── config_init.rs  ← avix config init implementation
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/atp_token.rs`:

```rust
use avix_core::auth::atp_token::{ATPToken, ATPTokenClaims};
use avix_core::types::Role;

fn signing_secret() -> &'static str { "test-secret-32-bytes-exactly-ok!!" }

// ── Issue and validate ────────────────────────────────────────────────────────

#[test]
fn issue_and_validate_token() {
    let claims = ATPTokenClaims {
        session_id:    "sess-001".into(),
        identity_name: "alice".into(),
        role:          Role::Admin,
        expires_at:    chrono::Utc::now() + chrono::Duration::hours(8),
    };
    let token = ATPToken::issue(claims.clone(), signing_secret()).unwrap();
    let validated = ATPToken::validate(&token, signing_secret()).unwrap();
    assert_eq!(validated.identity_name, "alice");
    assert_eq!(validated.session_id, "sess-001");
}

#[test]
fn validate_with_wrong_secret_fails() {
    let claims = ATPTokenClaims {
        session_id: "s".into(), identity_name: "alice".into(),
        role: Role::User, expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    };
    let token = ATPToken::issue(claims, signing_secret()).unwrap();
    assert!(ATPToken::validate(&token, "wrong-secret").is_err());
}

#[test]
fn validate_expired_token_fails() {
    let claims = ATPTokenClaims {
        session_id: "s".into(), identity_name: "alice".into(),
        role: Role::User,
        expires_at: chrono::Utc::now() - chrono::Duration::seconds(1), // already expired
    };
    let token = ATPToken::issue(claims, signing_secret()).unwrap();
    assert!(ATPToken::validate(&token, signing_secret()).is_err());
}

// ── Per-message revocation ────────────────────────────────────────────────────

#[tokio::test]
async fn revoked_token_fails_immediately() {
    use avix_core::auth::atp_token::ATPTokenStore;
    let store = ATPTokenStore::new(signing_secret().into());

    let claims = ATPTokenClaims {
        session_id: "sess-revoke".into(), identity_name: "bob".into(),
        role: Role::User, expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    };
    let token = store.issue(claims).await.unwrap();

    // First validation OK
    assert!(store.validate(&token).await.is_ok());

    // Revoke
    store.revoke("sess-revoke").await;

    // Next validation fails immediately
    assert!(store.validate(&token).await.is_err());
}

// ── Domain access ─────────────────────────────────────────────────────────────

#[test]
fn admin_token_can_access_sys_and_cap() {
    let claims = ATPTokenClaims {
        session_id: "s".into(), identity_name: "alice".into(),
        role: Role::Admin, expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    };
    let token = ATPToken::issue(claims, signing_secret()).unwrap();
    let validated = ATPToken::validate(&token, signing_secret()).unwrap();
    assert!(validated.role.can_access_domain("sys"));
    assert!(validated.role.can_access_domain("cap"));
}

#[test]
fn user_token_cannot_access_sys() {
    let claims = ATPTokenClaims {
        session_id: "s".into(), identity_name: "bob".into(),
        role: Role::User, expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
    };
    let token = ATPToken::issue(claims, signing_secret()).unwrap();
    let validated = ATPToken::validate(&token, signing_secret()).unwrap();
    assert!(!validated.role.can_access_domain("sys"));
}

// ── avix config init ──────────────────────────────────────────────────────────

#[test]
fn config_init_creates_auth_conf() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let params = ConfigInitParams {
        root:              tmp.path().to_path_buf(),
        identity_name:     "alice".into(),
        credential_type:   "api_key".into(),
        role:              "admin".into(),
        master_key_source: "env".into(),
        mode:              "cli".into(),
    };

    let result = run_config_init(params).unwrap();
    assert!(result.api_key.starts_with("sk-avix-"));
    assert!(tmp.path().join("etc/auth.conf").exists());
}

#[test]
fn config_init_idempotent_without_force() {
    use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    let params = || ConfigInitParams {
        root: tmp.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    };

    run_config_init(params()).unwrap();
    // Second call without force — no-op, no error
    let result = run_config_init(params());
    assert!(result.is_ok());
}

#[tokio::test]
async fn bootstrap_aborts_without_auth_conf() {
    use avix_core::bootstrap::Runtime;
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
    // No auth.conf created

    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("auth.conf"));
}
```

---

## Step 3 — Implement

**`src/auth/atp_token.rs`** — `ATPToken` is a base64url-encoded HMAC-SHA256-signed JSON claims blob. `ATPTokenStore` adds a revocation set (`HashSet<session_id>` in `RwLock`).

**`src/cli/config_init.rs`** — `run_config_init`: creates `AVIX_ROOT/etc/`, generates a random `sk-avix-<uuid>` API key, hashes it with HMAC-SHA256, writes `auth.conf` YAML. Returns the raw key to the caller (printed to stdout in CLI mode).

**`src/bootstrap/mod.rs`** — `Runtime::bootstrap_with_root` checks for `auth.conf` and returns `Err` if missing.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 30+ new tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-11: ATPToken HMAC signing, revocation, domain access, avix config init"
```

## Success Criteria

- [ ] 30+ tests pass
- [ ] Token with wrong secret fails
- [ ] Expired token fails
- [ ] Revoked session fails immediately (per-message)
- [ ] `sys`/`cap` domains require admin role
- [ ] `avix config init` creates `auth.conf` with a valid `sk-avix-` key
- [ ] Second `config init` without `--force` is a no-op
- [ ] Bootstrap aborts without `auth.conf`

---
---

