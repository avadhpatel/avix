use serde_json::{json, Value};

use crate::memfs::VfsPath;
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

pub fn read(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    // Secrets are never readable via VFS
    if path.starts_with("/secrets/") {
        return Err(SyscallError::Eperm(
            _ctx.caller_pid,
            "fs/read /secrets/".into(),
        ));
    }
    Ok(json!({ "path": path, "content": "" }))
}

pub fn write(ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path_str = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing content".into()))?;

    let path = VfsPath::parse(path_str)
        .map_err(|e| SyscallError::Einval(e.to_string()))?;
    if !path.is_agent_writable() {
        return Err(SyscallError::Eperm(
            ctx.caller_pid,
            format!("EPERM: {path_str} is kernel-owned and not writable by agents"),
        ));
    }

    Ok(json!({ "path": path_str, "bytes_written": content.len() }))
}

pub fn list(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    Ok(json!({ "path": path, "entries": [] }))
}

pub fn exists(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    Ok(json!({ "path": path, "exists": false }))
}

pub fn delete(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    Ok(json!({ "path": path, "deleted": true }))
}

pub fn watch(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing path".into()))?;
    Ok(json!({ "path": path, "watch_id": "watch-1" }))
}
