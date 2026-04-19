use serde_json::{json, Value};
use tracing::instrument;

use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

#[instrument(skip(params))]
pub fn save(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    Ok(json!({ "snapshot_id": "snap-1", "pid": pid }))
}

#[instrument(skip(params))]
pub fn restore(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let snapshot_id = params
        .get("snapshot_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing snapshot_id".into()))?;
    Ok(json!({ "snapshot_id": snapshot_id, "restored": true }))
}

#[instrument(skip(params))]
pub fn list(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params
        .get("pid")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))?;
    Ok(json!({ "pid": pid, "snapshots": [] }))
}

#[instrument(skip(params))]
pub fn delete(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let snapshot_id = params
        .get("snapshot_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing snapshot_id".into()))?;
    Ok(json!({ "snapshot_id": snapshot_id, "deleted": true }))
}
