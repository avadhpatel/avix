use std::path::Path;

use serde_json::{json, Value};
use tracing::instrument;

use crate::secrets::SecretStore;
use crate::service::installer::{InstallRequest, ServiceInstaller};
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

#[instrument(skip(_params))]
pub fn info(_ctx: &SyscallContext, _params: Value) -> SyscallResult {
    Ok(json!({
        "version": "0.1.0",
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "uptime_secs": 0
    }))
}

#[instrument(skip(params))]
pub fn boot_log(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);
    Ok(json!({ "lines": [], "limit": limit }))
}

#[instrument(skip(params))]
pub fn reboot(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let confirm = params
        .get("confirm")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| SyscallError::Einval("missing confirm".into()))?;
    if !confirm {
        return Err(SyscallError::Einval("reboot requires confirm: true".into()));
    }
    Ok(json!({ "rebooting": true }))
}

/// `kernel/secret/get` — retrieve an encrypted secret by owner + name.
///
/// Permission model:
/// - The caller must have `kernel/secret/get` in their granted tools.
/// - Service-owned secrets (`"service:<name>"`) can only be read by callers
///   whose token's `issued_to.agent_name` equals `<name>` or by admins (`auth:admin`).
/// - User-owned secrets (`"user:<name>"`) can be read by any authorised caller.
#[instrument(skip(params, secret_store))]
pub fn secret_get(
    ctx: &SyscallContext,
    params: Value,
    secret_store: &SecretStore,
) -> SyscallResult {
    let owner = params["owner"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing `owner`".into()))?;
    let name = params["name"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing `name`".into()))?;

    // Service-owned secret: only the owning service or an admin may read it.
    if let Some(svc_name) = owner.strip_prefix("service:") {
        let issued_agent = ctx
            .token
            .issued_to
            .as_ref()
            .map(|i| i.agent_name.as_str())
            .unwrap_or("");
        if issued_agent != svc_name && !ctx.token.has_tool("auth:admin") {
            return Err(SyscallError::Eperm(
                ctx.caller_pid,
                "kernel/secret/get".into(),
            ));
        }
    }

    let value = secret_store
        .get(owner, name)
        .map_err(|e| SyscallError::Enoent(e.to_string()))?;

    Ok(json!({ "value": value }))
}

#[instrument(skip(params))]
pub async fn install(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    if !ctx.token.has_tool("auth:admin") {
        return Err(SyscallError::Eperm(ctx.caller_pid, "sys/install".into()));
    }
    let source = params["source"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing `source`".into()))?
        .to_string();
    let checksum = params["checksum"].as_str().map(String::from);
    let autostart = params["autostart"].as_bool().unwrap_or(true);

    let installer = ServiceInstaller::new(avix_root.to_path_buf());
    let result = installer
        .install(InstallRequest {
            source,
            checksum,
            autostart,
        })
        .await
        .map_err(|e| SyscallError::Einval(e.to_string()))?;

    Ok(json!({
        "name":        result.name,
        "version":     result.version,
        "tools":       result.tools,
        "install_dir": result.install_dir.display().to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::CapabilityToken;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_ctx(tools: &[&str]) -> SyscallContext {
        SyscallContext {
            caller_pid: 42,
            token: CapabilityToken::test_token(tools),
        }
    }

    fn make_store(dir: &TempDir) -> SecretStore {
        SecretStore::new(dir.path(), b"test-master-key-32-bytes-padded!!")
    }

    #[test]
    fn secret_get_returns_value() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("user:alice", "gh-token", "ghp_xyz").unwrap();

        let ctx = make_ctx(&["kernel/secret/get"]);
        let result = secret_get(
            &ctx,
            json!({"owner": "user:alice", "name": "gh-token"}),
            &store,
        )
        .unwrap();
        assert_eq!(result["value"], "ghp_xyz");
    }

    #[test]
    fn secret_get_missing_owner_errs() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        let ctx = make_ctx(&["kernel/secret/get"]);
        let result = secret_get(&ctx, json!({"name": "key"}), &store);
        assert!(matches!(result, Err(SyscallError::Einval(_))));
    }

    #[test]
    fn secret_get_missing_name_errs() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        let ctx = make_ctx(&["kernel/secret/get"]);
        let result = secret_get(&ctx, json!({"owner": "user:alice"}), &store);
        assert!(matches!(result, Err(SyscallError::Einval(_))));
    }

    #[test]
    fn secret_get_nonexistent_returns_enoent() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        let ctx = make_ctx(&["kernel/secret/get"]);
        let result = secret_get(&ctx, json!({"owner": "user:alice", "name": "nope"}), &store);
        assert!(matches!(result, Err(SyscallError::Enoent(_))));
    }

    #[test]
    fn secret_get_service_secret_blocked_for_wrong_agent() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("service:github-svc", "key", "val").unwrap();

        let ctx = make_ctx(&["kernel/secret/get"]); // issued_to is None → not the service
        let result = secret_get(
            &ctx,
            json!({"owner": "service:github-svc", "name": "key"}),
            &store,
        );
        assert!(matches!(result, Err(SyscallError::Eperm(_, _))));
    }

    #[test]
    fn secret_get_service_secret_allowed_for_admin() {
        let dir = TempDir::new().unwrap();
        let store = make_store(&dir);
        store.set("service:github-svc", "key", "val").unwrap();

        let ctx = make_ctx(&["kernel/secret/get", "auth:admin"]);
        let result = secret_get(
            &ctx,
            json!({"owner": "service:github-svc", "name": "key"}),
            &store,
        )
        .unwrap();
        assert_eq!(result["value"], "val");
    }
}
