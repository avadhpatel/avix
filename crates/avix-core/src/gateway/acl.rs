use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::types::AtpDomain;
use crate::types::Role;

/// Check that `caller` meets the minimum role required for `domain` / `op` (§3.2, §6).
pub fn check_domain_role(domain: AtpDomain, op: &str, caller: Role) -> Result<(), AtpError> {
    let min_role = domain_min_role(domain, op);
    if caller < min_role {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("role '{caller}' cannot invoke domain '{domain:?}' op '{op}'"),
        ));
    }
    Ok(())
}

/// Determines the minimum role required to invoke operations on a given ATP domain (§6).
/// This enforces access control based on domain sensitivity and operation type.
/// For domains with operation-specific roles, matches on `op`; otherwise defaults to a fixed role.
/// Tracing logs every call for audit and debugging.
/// Links: [ATP Spec §6](https://atproto.com/specs/atp)
fn domain_min_role(domain: AtpDomain, op: &str) -> Role {
    tracing::debug!("domain_min_role: domain={:?}, op={}", domain, op);
    match domain {
        AtpDomain::Auth => Role::Guest,
        AtpDomain::Proc => match op {
            "list" => Role::Guest,
            "setcap" => Role::Operator,
            _ => Role::User,
        },
        AtpDomain::Signal => Role::User,
        AtpDomain::Fs => match op {
            "read" | "list" | "stat" => Role::Guest,
            _ => Role::User,
        },
        AtpDomain::Snap => match op {
            "delete" => Role::Operator,
            _ => Role::User,
        },
        AtpDomain::Cron => Role::User,
        AtpDomain::Users => match op {
            "list" => Role::Operator,
            "create" | "update" | "delete" => Role::Admin,
            _ => Role::User, // get, passwd
        },
        AtpDomain::Crews => match op {
            "create" | "update" | "delete" | "join" | "leave" => Role::Admin,
            _ => Role::Guest,
        },
        AtpDomain::Cap => match op {
            "inspect" => Role::Operator,
            _ => Role::Admin,
        },
        AtpDomain::Sys => match op {
            // Read-only observability ops — any authenticated user can query these.
            "status" | "logs" | "service-list" | "tool-list" => Role::User,
            // Service lifecycle ops require operator.
            "install" | "uninstall" | "update" => Role::Operator,
            // Mutating / destructive ops require admin.
            _ => Role::Admin,
        },
        AtpDomain::Pipe => Role::User,
        AtpDomain::Session => Role::User,
    }
}

/// Check that `caller` is permitted to access a resource owned by `target_owner`.
/// Users may only target their own resources; `Operator+` may target any.
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
        format!("'{caller_identity}' cannot access resources owned by '{target_owner}'"),
    ))
}

/// Hard-veto writes to `/secrets/` or `/proc/` regardless of role (§10 rules 3 & 4).
pub fn check_fs_hard_veto(path: &str, op: &str) -> Result<(), AtpError> {
    if op != "write" {
        return Ok(());
    }
    if path.starts_with(r#"/secrets/"#) || path.starts_with(r#"/proc/"#) {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("writes to '{path}' are unconditionally forbidden"),
        ));
    }
    Ok(())
}

/// Some domains/ops are only accessible from the admin port (7701).
/// `is_admin_port` is set by `GatewayServer` based on which listener accepted the connection.
pub fn check_admin_port(domain: AtpDomain, op: &str, is_admin_port: bool) -> Result<(), AtpError> {
    let requires_admin_port = match domain {
        AtpDomain::Cap => true,
        // read-only sys ops are allowed on user port; mutating ones require admin port
        AtpDomain::Sys => !matches!(op, "status" | "logs" | "service-list" | "tool-list"),
        _ => false,
    };
    if requires_admin_port && !is_admin_port {
        return Err(AtpError::new(
            AtpErrorCode::Eperm,
            format!("domain '{domain:?}' op '{op}' requires admin port 7701"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── domain × role matrix ───────────────────────────────────────────────────

    #[test]
    fn guest_can_list_proc() {
        assert!(check_domain_role(AtpDomain::Proc, "list", Role::Guest).is_ok());
    }

    #[test]
    fn guest_cannot_spawn() {
        assert!(check_domain_role(AtpDomain::Proc, "spawn", Role::Guest).is_err());
    }

    #[test]
    fn user_can_spawn() {
        assert!(check_domain_role(AtpDomain::Proc, "spawn", Role::User).is_ok());
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
    fn guest_can_stat_fs() {
        assert!(check_domain_role(AtpDomain::Fs, "stat", Role::Guest).is_ok());
    }

    #[test]
    fn user_cannot_list_users() {
        assert!(check_domain_role(AtpDomain::Users, "list", Role::User).is_err());
    }

    #[test]
    fn operator_can_list_users() {
        assert!(check_domain_role(AtpDomain::Users, "list", Role::Operator).is_ok());
    }

    #[test]
    fn user_cannot_create_user() {
        assert!(check_domain_role(AtpDomain::Users, "create", Role::User).is_err());
    }

    #[test]
    fn admin_can_create_user() {
        assert!(check_domain_role(AtpDomain::Users, "create", Role::Admin).is_ok());
    }

    #[test]
    fn admin_can_delete_user() {
        assert!(check_domain_role(AtpDomain::Users, "delete", Role::Admin).is_ok());
    }

    #[test]
    fn user_can_get_own_user() {
        assert!(check_domain_role(AtpDomain::Users, "get", Role::User).is_ok());
    }

    #[test]
    fn guest_can_list_crews() {
        assert!(check_domain_role(AtpDomain::Crews, "list", Role::Guest).is_ok());
    }

    #[test]
    fn user_cannot_create_crew() {
        assert!(check_domain_role(AtpDomain::Crews, "create", Role::User).is_err());
    }

    #[test]
    fn admin_can_create_crew() {
        assert!(check_domain_role(AtpDomain::Crews, "create", Role::Admin).is_ok());
    }

    #[test]
    fn operator_can_inspect_cap() {
        assert!(check_domain_role(AtpDomain::Cap, "inspect", Role::Operator).is_ok());
    }

    #[test]
    fn operator_cannot_grant_cap() {
        assert!(check_domain_role(AtpDomain::Cap, "grant", Role::Operator).is_err());
    }

    #[test]
    fn admin_can_grant_cap() {
        assert!(check_domain_role(AtpDomain::Cap, "grant", Role::Admin).is_ok());
    }

    #[test]
    fn user_can_view_sys_status() {
        assert!(check_domain_role(AtpDomain::Sys, "status", Role::User).is_ok());
    }

    #[test]
    fn user_can_list_services() {
        assert!(check_domain_role(AtpDomain::Sys, "service-list", Role::User).is_ok());
    }

    #[test]
    fn user_can_list_tools() {
        assert!(check_domain_role(AtpDomain::Sys, "tool-list", Role::User).is_ok());
    }

    #[test]
    fn guest_cannot_list_services() {
        assert!(check_domain_role(AtpDomain::Sys, "service-list", Role::Guest).is_err());
    }

    #[test]
    fn user_cannot_shutdown() {
        assert!(check_domain_role(AtpDomain::Sys, "shutdown", Role::User).is_err());
    }

    #[test]
    fn admin_can_shutdown() {
        assert!(check_domain_role(AtpDomain::Sys, "shutdown", Role::Admin).is_ok());
    }

    #[test]
    fn user_can_open_pipe() {
        assert!(check_domain_role(AtpDomain::Pipe, "open", Role::User).is_ok());
    }

    #[test]
    fn user_can_manage_session() {
        assert!(check_domain_role(AtpDomain::Session, "create", Role::User).is_ok());
    }

    #[test]
    fn guest_cannot_send_signal() {
        assert!(check_domain_role(AtpDomain::Signal, "send", Role::Guest).is_err());
    }

    #[test]
    fn user_can_delete_own_snap() {
        assert!(check_domain_role(AtpDomain::Snap, "create", Role::User).is_ok());
    }

    #[test]
    fn user_cannot_delete_snap() {
        assert!(check_domain_role(AtpDomain::Snap, "delete", Role::User).is_err());
    }

    #[test]
    fn operator_can_delete_snap() {
        assert!(check_domain_role(AtpDomain::Snap, "delete", Role::Operator).is_ok());
    }

    // ── ownership ─────────────────────────────────────────────────────────────

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

    #[test]
    fn admin_can_target_any_resource() {
        assert!(check_ownership("alice", Role::Admin, "bob").is_ok());
    }

    // ── hard vetoes ───────────────────────────────────────────────────────────

    #[test]
    fn write_to_secrets_always_blocked() {
        assert!(check_fs_hard_veto(r#"/secrets/api_key"#, "write").is_err());
    }

    #[test]
    fn write_to_proc_always_blocked() {
        assert!(check_fs_hard_veto(r#"/proc/57/status.yaml"#, "write").is_err());
    }

    #[test]
    fn read_from_secrets_not_vetoed_here() {
        // /secrets/ read EPERM is enforced by VFS, not this check
        assert!(check_fs_hard_veto(r#"/secrets/api_key"#, "read").is_ok());
    }

    #[test]
    fn write_to_users_not_vetoed() {
        assert!(check_fs_hard_veto(r#"/users/alice/data.yaml"#, "write").is_ok());
    }

    #[test]
    fn read_from_proc_not_vetoed() {
        assert!(check_fs_hard_veto(r#"/proc/57/status.yaml"#, "read").is_ok());
    }

    // ── admin port ────────────────────────────────────────────────────────────

    #[test]
    fn cap_domain_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Cap, "grant", false).is_err());
    }

    #[test]
    fn cap_domain_allowed_on_admin_port() {
        assert!(check_admin_port(AtpDomain::Cap, "grant", true).is_ok());
    }

    #[test]
    fn cap_inspect_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Cap, "inspect", false).is_err());
    }

    #[test]
    fn sys_status_allowed_on_user_port() {
        assert!(check_admin_port(AtpDomain::Sys, "status", false).is_ok());
    }

    #[test]
    fn sys_logs_allowed_on_user_port() {
        assert!(check_admin_port(AtpDomain::Sys, "logs", false).is_ok());
    }

    #[test]
    fn sys_service_list_allowed_on_user_port() {
        assert!(check_admin_port(AtpDomain::Sys, "service-list", false).is_ok());
    }

    #[test]
    fn sys_tool_list_allowed_on_user_port() {
        assert!(check_admin_port(AtpDomain::Sys, "tool-list", false).is_ok());
    }

    #[test]
    fn sys_shutdown_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Sys, "shutdown", false).is_err());
    }

    #[test]
    fn sys_reload_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Sys, "reload", false).is_err());
    }

    #[test]
    fn sys_shutdown_allowed_on_admin_port() {
        assert!(check_admin_port(AtpDomain::Sys, "shutdown", true).is_ok());
    }

    #[test]
    fn proc_domain_never_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Proc, "spawn", false).is_ok());
    }

    #[test]
    fn fs_domain_never_requires_admin_port() {
        assert!(check_admin_port(AtpDomain::Fs, "write", false).is_ok());
    }
}
