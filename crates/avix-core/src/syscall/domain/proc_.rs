use serde_json::{json, Value};

use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

pub fn spawn(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;
    Ok(json!({ "pid": 100, "name": name, "status": "running" }))
}

pub fn kill(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    Ok(json!({ "killed": pid }))
}

pub fn list(_ctx: &SyscallContext, _params: Value) -> SyscallResult {
    Ok(json!({ "processes": [] }))
}

pub fn info(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    Ok(json!({ "pid": pid, "status": "running" }))
}

pub fn wait(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    Ok(json!({ "pid": pid, "exit_code": 0 }))
}

pub fn signal(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    let sig = params
        .get("signal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing signal".into()))?;
    Ok(json!({ "delivered": true, "pid": pid, "signal": sig }))
}
