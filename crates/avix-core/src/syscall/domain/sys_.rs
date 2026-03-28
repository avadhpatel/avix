use std::path::Path;

use serde_json::{json, Value};

use crate::service::installer::{InstallRequest, ServiceInstaller};
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

pub fn info(_ctx: &SyscallContext, _params: Value) -> SyscallResult {
    Ok(json!({
        "version": "0.1.0",
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "uptime_secs": 0
    }))
}

pub fn boot_log(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);
    Ok(json!({ "lines": [], "limit": limit }))
}

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

pub async fn install(
    ctx: &SyscallContext,
    params: Value,
    avix_root: &Path,
) -> SyscallResult {
    if !ctx.token.has_tool("auth:admin") {
        return Err(SyscallError::Eperm(
            ctx.caller_pid,
            "sys/install".into(),
        ));
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
