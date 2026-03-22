use serde_json::{json, Value};

use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

pub fn issue(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let target_pid = params
        .get("target_pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing target_pid".into()))?;
    let tools = params
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "token_id": "cap-token-1",
        "target_pid": target_pid,
        "granted_tools": tools
    }))
}

pub fn validate(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let token_id = params
        .get("token_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing token_id".into()))?;
    Ok(json!({ "token_id": token_id, "valid": true }))
}

pub fn revoke(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let token_id = params
        .get("token_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing token_id".into()))?;
    Ok(json!({ "token_id": token_id, "revoked": true }))
}

pub fn policy(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing action".into()))?;
    Ok(json!({ "action": action, "policy": "allow" }))
}
