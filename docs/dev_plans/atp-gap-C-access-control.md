# ATP Gap C — Access Control Pipeline

> **Spec reference:** §8 Access Control Model, §3.2 Role Hierarchy, §10 Security Rules
> **Priority:** Critical — must be in place before transport is wired up
> **Depends on:** ATP Gap A (AtpDomain, AtpErrorCode), ATP Gap B (ATPTokenClaims, SessionEntry)

---

## Problem

The existing translator does only one role check (`SysReboot` → admin only). The spec
requires an 8-step validation pipeline on every inbound command:

```
1. Parse JSON frame
2. Verify HMAC signature          → EAUTH if invalid
3. Check token expiry             → EEXPIRED if expired
4. Confirm sessionId matches conn → ESESSION if mismatch
5. Check domain × role matrix     → EPERM if denied
6. Check target resource ownership → EPERM if user targets other's PID
7. Hard veto /secrets/ writes     → EPERM unconditionally
8. Hard veto /proc/ writes        → EPERM unconditionally
```

Plus security rules:
- **Replay protection** — duplicate command `id` within a session → `EPARSE`
- **Admin port flag** — `sys` (mutating ops) and `cap` domain only on port 7701

There is also no per-domain × role access matrix anywhere in the codebase.

---

## What to Build

### 1. Domain × Role access matrix

File: `crates/avix-core/src/gateway/acl.rs`

```rust
use crate::gateway::atp::types::AtpDomain;
use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::types::Role;

/// Minimum role required to use each domain, per spec §3.2.
/// Returns `Err(AtpError::Eperm)` if `caller` is below the minimum.
pub fn check_domain_role(
    domain: AtpDomain,
    op: &str,
    caller: Role,
) -> Result<(), AtpError> {
    let min_role = domain_min_role(domain, op);
    if caller < min_role {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("Role '{:?}' cannot invoke domain '{:?}' op '{}'", caller, domain, op),
        ));
    }
    Ok(())
}

fn domain_min_role(domain: AtpDomain, op: &str) -> Role {
    match domain {
        AtpDomain::Auth    => Role::Guest,    // all ops: guest+
        AtpDomain::Proc    => match op {
            "list"  => Role::Guest,
            "setcap" => Role::Operator,
            _       => Role::User,
        },
        AtpDomain::Signal  => Role::User,
        AtpDomain::Fs      => match op {
            "read" | "list" | "stat" => Role::Guest,
            _                       => Role::User,
        },
        AtpDomain::Snap    => match op {
            "delete" => Role::Operator,
            _        => Role::User,
        },
        AtpDomain::Cron    => Role::User,
        AtpDomain::Users   => match op {
            "list"            => Role::Operator,
            "create" | "update" | "delete" => Role::Admin,
            _                 => Role::User,    // get, passwd
        },
        AtpDomain::Crews   => match op {
            "create" | "update" | "delete" | "join" | "leave" => Role::Admin,
            _                                                  => Role::Guest,
        },
        AtpDomain::Cap     => match op {
            "inspect" => Role::Operator,
            _         => Role::Admin,
        },
        AtpDomain::Sys     => match op {
            "status" | "logs" | "install" | "uninstall" | "update" => Role::Operator,
            _                                                       => Role::Admin,
        },
        AtpDomain::Pipe    => Role::User,
    }
}
```

**Note:** `Role` must implement `PartialOrd` with the ordering `Guest < User < Operator < Admin`.
Check `crates/avix-core/src/types/role.rs` and add `#[derive(PartialOrd, Ord)]` if missing.

### 2. Ownership check

File: `crates/avix-core/src/gateway/acl.rs` (add to same file)

```rust
/// Check that `caller` is allowed to target `target_owner`.
/// Users may only target their own resources; operators and admins may target any.
pub fn check_ownership(
    caller_identity: &str,
    caller_role: Role,
    target_owner: &str,
) -> Result<(), AtpError> {
    if caller_role >= Role::Operator {
        return Ok(());
    }
    if caller_identity == target_owner {
        return Ok(());
    }
    Err(AtpError::new(
        AtpErrorCode::Eperm,
        format!("'{}' cannot access resources owned by '{}'", caller_identity, target_owner),
    ))
}
```

### 3. Hard path vetoes

File: `crates/avix-core/src/gateway/acl.rs` (add to same file)

```rust
/// Hard-veto writes to /secrets/ or /proc/ regardless of role (§10 rules 3 & 4).
pub fn check_fs_hard_veto(path: &str, op: &str) -> Result<(), AtpError> {
    if op != "write" {
        return Ok(());
    }
    if path.starts_with("/secrets/") || path.starts_with("/proc/") {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("writes to '{}' are unconditionally forbidden", path),
        ));
    }
    Ok(())
}
```

### 4. Admin-port domain check

File: `crates/avix-core/src/gateway/acl.rs`

```rust
/// Some domains/ops are only accessible from port 7701 (admin port).
/// `is_admin_port` is set by the GatewayServer based on which listener accepted the conn.
pub fn check_admin_port(domain: AtpDomain, op: &str, is_admin_port: bool) -> Result<(), AtpError> {
    let requires_admin_port = match domain {
        AtpDomain::Cap => true,
        AtpDomain::Sys => !matches!(op, "status" | "logs"), // read-only sys ops allowed on 7700
        _ => false,
    };
    if requires_admin_port && !is_admin_port {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("domain '{:?}' op '{}' requires admin port 7701", domain, op),
        ));
    }
    Ok(())
}
```

### 5. Replay protection

File: `crates/avix-core/src/gateway/replay.rs`

```rust
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::gateway::atp::error::{AtpError, AtpErrorCode};

/// Per-connection replay guard. Tracks seen command IDs.
/// Created fresh for each WebSocket connection.
#[derive(Default, Clone)]
pub struct ReplayGuard {
    seen: Arc<Mutex<HashSet<String>>>,
}

impl ReplayGuard {
    pub fn new() -> Self { Self::default() }

    /// Register a command ID. Returns Err(EPARSE) if already seen.
    pub async fn check_and_register(&self, id: &str) -> Result<(), AtpError> {
        let mut guard = self.seen.lock().await;
        if guard.contains(id) {
            return Err(AtpError::new(
                AtpErrorCode::Eparse,
                format!("duplicate command id '{}'", id),
            ));
        }
        guard.insert(id.to_string());
        Ok(())
    }
}
```

### 6. `AtpValidator` — full 8-step pipeline

File: `crates/avix-core/src/gateway/validator.rs`

```rust
use crate::auth::atp_token::ATPTokenStore;
use crate::gateway::acl::{check_admin_port, check_domain_role, check_fs_hard_veto, check_ownership};
use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpCmd;
use crate::gateway::atp::types::AtpDomain;
use crate::gateway::replay::ReplayGuard;

pub struct ValidationContext<'a> {
    pub token_store: &'a ATPTokenStore,
    pub replay_guard: &'a ReplayGuard,
    pub session_id: &'a str,      // session_id bound to this WS connection
    pub is_admin_port: bool,
}

pub struct ValidatedCmd {
    pub cmd: AtpCmd,
    pub caller_identity: String,
    pub caller_role: crate::types::Role,
}

impl<'a> ValidationContext<'a> {
    /// Run all 8 validation steps. Returns a `ValidatedCmd` on success.
    pub async fn validate(&self, cmd: AtpCmd) -> Result<ValidatedCmd, AtpError> {
        // Step 2+3: HMAC + expiry (done inside token_store.validate)
        let claims = self.token_store.validate(&cmd.token).await.map_err(|e| {
            if e.to_string().contains("expired") {
                AtpError::new(AtpErrorCode::Eexpired, "token expired")
            } else {
                AtpError::new(AtpErrorCode::Eauth, format!("invalid token: {e}"))
            }
        })?;

        // Step 4: session ID must match this connection
        if claims.session_id != self.session_id {
            return Err(AtpError::new(AtpErrorCode::Esession, "session ID mismatch"));
        }

        // Step 5: domain × role matrix
        check_domain_role(cmd.domain, &cmd.op, claims.role)?;

        // Step 6: admin-port gate
        check_admin_port(cmd.domain, &cmd.op, self.is_admin_port)?;

        // Step 7+8: fs hard vetoes
        if cmd.domain == AtpDomain::Fs {
            let path = cmd.body["path"].as_str().unwrap_or("");
            check_fs_hard_veto(path, &cmd.op)?;
        }

        // Replay protection
        self.replay_guard.check_and_register(&cmd.id).await?;

        Ok(ValidatedCmd {
            caller_identity: claims.sub.clone(),
            caller_role: claims.role,
            cmd,
        })
    }
}
```

---

## Tests to Write

File: `crates/avix-core/src/gateway/acl.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Domain × Role matrix tests
    #[test]
    fn guest_can_read_proc_list() {
        assert!(check_domain_role(AtpDomain::Proc, "list", Role::Guest).is_ok());
    }

    #[test]
    fn guest_cannot_spawn() {
        assert!(check_domain_role(AtpDomain::Proc, "spawn", Role::Guest).is_err());
    }

    #[test]
    fn user_cannot_setcap() {
        assert!(check_domain_role(AtpDomain::Proc, "setcap", Role::User).is_err());
    }

    #[test]
    fn operator_can_setcap() {
        assert!(check_domain_role(AtpDomain::Proc, "setcap", Role::Operator).is_ok());
    }

    #[test]
    fn guest_cannot_write_fs() {
        assert!(check_domain_role(AtpDomain::Fs, "write", Role::Guest).is_err());
    }

    #[test]
    fn guest_can_read_fs() {
        assert!(check_domain_role(AtpDomain::Fs, "read", Role::Guest).is_ok());
    }

    #[test]
    fn user_cannot_create_user() {
        assert!(check_domain_role(AtpDomain::Users, "create", Role::User).is_err());
    }

    #[test]
    fn admin_can_delete_user() {
        assert!(check_domain_role(AtpDomain::Users, "delete", Role::Admin).is_ok());
    }

    // Ownership tests
    #[test]
    fn user_can_target_own_resource() {
        assert!(check_ownership("alice", Role::User, "alice").is_ok());
    }

    #[test]
    fn user_cannot_target_other_resource() {
        assert!(check_ownership("alice", Role::User, "bob").is_err());
    }

    #[test]
    fn operator_can_target_any_resource() {
        assert!(check_ownership("alice", Role::Operator, "bob").is_ok());
    }

    // Hard veto tests
    #[test]
    fn write_to_secrets_always_blocked() {
        assert!(check_fs_hard_veto("/secrets/api_key", "write").is_err());
    }

    #[test]
    fn write_to_proc_always_blocked() {
        assert!(check_fs_hard_veto("/proc/57/status.yaml", "write").is_err());
    }

    #[test]
    fn read_from_secrets_allowed_by_veto_check() {
        // hard veto only applies to writes; VFS read of /secrets/ returns EPERM via a different path
        assert!(check_fs_hard_veto("/secrets/api_key", "read").is_ok());
    }

    #[test]
    fn write_to_users_not_vetoed() {
        assert!(check_fs_hard_veto("/users/alice/data.yaml", "write").is_ok());
    }

    // Admin port tests
    #[test]
    fn cap_domain_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Cap, "grant", false).is_err());
    }

    #[test]
    fn cap_domain_allowed_on_admin_port() {
        assert!(check_admin_port(AtpDomain::Cap, "grant", true).is_ok());
    }

    #[test]
    fn sys_status_allowed_on_user_port() {
        assert!(check_admin_port(AtpDomain::Sys, "status", false).is_ok());
    }

    #[test]
    fn sys_shutdown_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Sys, "shutdown", false).is_err());
    }

    #[test]
    fn proc_domain_never_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Proc, "spawn", false).is_ok());
    }
}
```

File: `crates/avix-core/src/gateway/replay.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_id_accepted() {
        let guard = ReplayGuard::new();
        assert!(guard.check_and_register("c-001").await.is_ok());
    }

    #[tokio::test]
    async fn duplicate_id_rejected() {
        let guard = ReplayGuard::new();
        guard.check_and_register("c-001").await.unwrap();
        let err = guard.check_and_register("c-001").await.unwrap_err();
        assert_eq!(err.code, crate::gateway::atp::error::AtpErrorCode::Eparse);
    }

    #[tokio::test]
    async fn different_ids_both_accepted() {
        let guard = ReplayGuard::new();
        assert!(guard.check_and_register("c-001").await.is_ok());
        assert!(guard.check_and_register("c-002").await.is_ok());
    }
}
```

---

## Success Criteria

- [ ] `Role` implements `PartialOrd` / `Ord` with correct hierarchy (`Guest < User < Operator < Admin`)
- [ ] `check_domain_role` enforces the full domain × role matrix for all 11 domains
- [ ] `check_ownership` blocks cross-user access for `Role::User`; permits for `Operator+`
- [ ] `check_fs_hard_veto` blocks `/secrets/` and `/proc/` writes unconditionally
- [ ] `check_admin_port` blocks `cap` and mutating `sys` ops on port 7700
- [ ] `ReplayGuard::check_and_register` rejects duplicate command IDs with `EPARSE`
- [ ] `AtpValidator::validate` chains all steps in order; short-circuits on first failure
- [ ] All above tests pass; `cargo clippy` zero warnings
