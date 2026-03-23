# ATP Gap B — Token & Session Alignment

> **Spec reference:** §3 Authentication and Session Tokens, §4 Connection Lifecycle
> **Priority:** Critical
> **Depends on:** ATP Gap A (AtpErrorCode)

---

## Problem

`ATPTokenClaims` and `SessionEntry` are missing fields required by the spec. Token
encoding uses hex (not base64url). There is no token refresh pathway and no
`token.expiring` event.

### Missing from `ATPTokenClaims`

| Spec field   | Current | Gap |
|-------------|---------|-----|
| `sub`       | `identity_name` | rename + keep |
| `uid`       | missing | add `u32` |
| `role`      | present | OK |
| `crews`     | missing | `Vec<String>` |
| `sessionId` | `session_id` | rename |
| `iat`       | missing | `DateTime<Utc>` |
| `exp`       | `expires_at` | rename |
| `scope`     | missing | `Vec<String>` (domain names) |

### Missing from `SessionEntry`

| Spec requirement | Current | Gap |
|----------------|---------|-----|
| `uid: u32`     | missing | add |
| `state`        | missing | `active \| idle \| closed` |
| `agents: Vec<Pid>` | missing | add |
| `connected_at` | missing | `DateTime<Utc>` |
| `last_activity_at` | missing | `DateTime<Utc>` |
| `closed_at` / `closed_reason` | missing | for closed state |

### Token encoding

Current `base64_encode` is actually hex encoding. Spec requires base64url (standard JWT
encoding). The `base64` crate's `URL_SAFE_NO_PAD` alphabet is the target.

---

## What to Build

### 1. Align `ATPTokenClaims`

File: `crates/avix-core/src/auth/atp_token.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::Role;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ATPTokenClaims {
    pub sub: String,              // username (was: identity_name)
    pub uid: u32,
    pub role: Role,
    pub crews: Vec<String>,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub iat: DateTime<Utc>,
    pub exp: DateTime<Utc>,       // was: expires_at
    pub scope: Vec<String>,       // allowed domain names e.g. ["proc","fs","signal"]
}

impl ATPTokenClaims {
    pub fn is_expired(&self) -> bool {
        self.exp < Utc::now()
    }

    /// True if < 5 minutes remain before expiry.
    pub fn is_expiring_soon(&self) -> bool {
        let remaining = self.exp.signed_duration_since(Utc::now());
        remaining < chrono::Duration::minutes(5) && remaining > chrono::Duration::zero()
    }
}
```

**Fix `ATPToken::issue` / `validate`** to use `base64::engine::general_purpose::URL_SAFE_NO_PAD`
from the `base64` crate. Remove the manual hex functions.

### 2. Align `SessionEntry` + add `SessionState`

File: `crates/avix-core/src/auth/session.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::{Pid, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Active,
    Idle,   // reconnect grace window
    Closed,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    pub identity_name: String,
    pub uid: u32,
    pub role: Role,
    pub crews: Vec<String>,
    pub scope: Vec<String>,
    pub state: SessionState,
    pub connected_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub idle_since: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_reason: Option<String>,
    pub agents: Vec<Pid>,
}

impl SessionEntry {
    /// True if the reconnect grace window (60 s) has expired while idle.
    pub fn grace_expired(&self) -> bool {
        match self.idle_since {
            Some(t) => Utc::now().signed_duration_since(t) > chrono::Duration::seconds(60),
            None => false,
        }
    }

    pub fn mark_idle(&mut self) {
        self.state = SessionState::Idle;
        self.idle_since = Some(Utc::now());
    }

    pub fn mark_active(&mut self) {
        self.state = SessionState::Active;
        self.idle_since = None;
        self.last_activity_at = Utc::now();
    }

    pub fn mark_closed(&mut self, reason: impl Into<String>) {
        self.state = SessionState::Closed;
        self.closed_at = Some(Utc::now());
        self.closed_reason = Some(reason.into());
    }
}
```

### 3. Token refresh in `AuthService`

File: `crates/avix-core/src/auth/service.rs`

Add method:

```rust
/// Validate the presented token and issue a fresh one with a new expiry.
/// Returns (new_token_string, new_claims).
pub async fn refresh_token(
    &self,
    old_token: &str,
    token_store: &ATPTokenStore,
) -> Result<(String, ATPTokenClaims), AvixError> {
    let claims = token_store.validate(old_token).await?;
    // Session must still be valid
    self.validate_session(&claims.session_id).await?;
    let new_claims = ATPTokenClaims {
        iat: Utc::now(),
        exp: Utc::now() + chrono::Duration::hours(8),
        ..claims
    };
    let new_token = token_store.issue(new_claims.clone()).await?;
    Ok((new_token, new_claims))
}
```

### 4. `token.expiring` background task helper

Add to `ATPTokenStore`:

```rust
/// Returns true if the token will expire within 5 minutes.
/// The gateway connection loop calls this after every successful validation
/// and pushes `token.expiring` to the client if true.
pub async fn is_expiring_soon(&self, token: &str) -> Result<bool, AvixError> {
    let claims = self.validate(token).await?;
    Ok(claims.is_expiring_soon())
}
```

The actual event push is wired in ATP Gap D (transport layer) — this gap only provides
the predicate.

---

## Tests to Write

File: `crates/avix-core/src/auth/atp_token.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn sample_claims(exp_offset_secs: i64) -> ATPTokenClaims {
        ATPTokenClaims {
            sub: "alice".into(),
            uid: 1001,
            role: crate::types::Role::User,
            crews: vec!["researchers".into()],
            session_id: "sess-001".into(),
            iat: Utc::now(),
            exp: Utc::now() + Duration::seconds(exp_offset_secs),
            scope: vec!["proc".into(), "fs".into()],
        }
    }

    #[test]
    fn issue_and_validate_roundtrip() {
        let claims = sample_claims(3600);
        let token = ATPToken::issue(claims.clone(), "secret").unwrap();
        let validated = ATPToken::validate(&token, "secret").unwrap();
        assert_eq!(validated.sub, "alice");
        assert_eq!(validated.uid, 1001);
        assert_eq!(validated.crews, vec!["researchers"]);
        assert_eq!(validated.scope, vec!["proc", "fs"]);
    }

    #[test]
    fn expired_token_fails_validation() {
        let claims = sample_claims(-1);
        let token = ATPToken::issue(claims, "secret").unwrap();
        assert!(ATPToken::validate(&token, "secret").is_err());
    }

    #[test]
    fn wrong_secret_fails_validation() {
        let claims = sample_claims(3600);
        let token = ATPToken::issue(claims, "secret").unwrap();
        assert!(ATPToken::validate(&token, "wrong").is_err());
    }

    #[test]
    fn tampered_payload_fails_validation() {
        let claims = sample_claims(3600);
        let token = ATPToken::issue(claims, "secret").unwrap();
        // flip a char in the payload part
        let mut parts: Vec<String> = token.splitn(2, '.').map(String::from).collect();
        let bytes = parts[0].as_bytes_mut();
        bytes[4] ^= 0x01;
        let tampered = format!("{}.{}", parts[0], parts[1]);
        assert!(ATPToken::validate(&tampered, "secret").is_err());
    }

    #[test]
    fn token_uses_base64url_encoding() {
        let claims = sample_claims(3600);
        let token = ATPToken::issue(claims, "secret").unwrap();
        let payload_part = token.split('.').next().unwrap();
        // base64url alphabet: A-Z a-z 0-9 - _
        assert!(payload_part.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn is_expiring_soon_within_5_minutes() {
        let claims = sample_claims(240); // 4 min
        assert!(claims.is_expiring_soon());
    }

    #[test]
    fn is_expiring_soon_beyond_5_minutes() {
        let claims = sample_claims(600); // 10 min
        assert!(!claims.is_expiring_soon());
    }

    #[tokio::test]
    async fn store_revoke_makes_validate_fail() {
        let store = ATPTokenStore::new("secret".into());
        let claims = sample_claims(3600);
        let token = store.issue(claims).await.unwrap();
        store.revoke("sess-001").await;
        assert!(store.validate(&token).await.is_err());
    }
}
```

File: `crates/avix-core/src/auth/session.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_entry() -> SessionEntry {
        SessionEntry {
            session_id: "s-001".into(),
            identity_name: "alice".into(),
            uid: 1001,
            role: crate::types::Role::User,
            crews: vec![],
            scope: vec!["proc".into()],
            state: SessionState::Active,
            connected_at: Utc::now(),
            last_activity_at: Utc::now(),
            idle_since: None,
            closed_at: None,
            closed_reason: None,
            agents: vec![],
        }
    }

    #[test]
    fn mark_idle_sets_state_and_timestamp() {
        let mut e = make_entry();
        e.mark_idle();
        assert_eq!(e.state, SessionState::Idle);
        assert!(e.idle_since.is_some());
    }

    #[test]
    fn mark_active_clears_idle() {
        let mut e = make_entry();
        e.mark_idle();
        e.mark_active();
        assert_eq!(e.state, SessionState::Active);
        assert!(e.idle_since.is_none());
    }

    #[test]
    fn grace_not_expired_immediately_after_idle() {
        let mut e = make_entry();
        e.mark_idle();
        assert!(!e.grace_expired());
    }

    #[test]
    fn mark_closed_sets_reason() {
        let mut e = make_entry();
        e.mark_closed("ping timeout");
        assert_eq!(e.state, SessionState::Closed);
        assert_eq!(e.closed_reason.as_deref(), Some("ping timeout"));
    }
}
```

---

## Success Criteria

- [ ] `ATPTokenClaims` has all 8 spec fields (`sub`, `uid`, `role`, `crews`, `sessionId`, `iat`, `exp`, `scope`)
- [ ] Token encoding uses base64url (not hex)
- [ ] `SessionEntry` has `uid`, `state`, `agents`, `connected_at`, `last_activity_at`
- [ ] `SessionEntry::mark_idle` / `mark_active` / `mark_closed` / `grace_expired` work correctly
- [ ] `ATPTokenStore::is_expiring_soon` returns true when < 5 min remain
- [ ] `AuthService::refresh_token` issues a new token preserving all claims except `iat`/`exp`
- [ ] All above tests pass; `cargo clippy` zero warnings
