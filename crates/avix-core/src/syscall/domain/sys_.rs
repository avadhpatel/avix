use serde_json::{json, Value};

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
