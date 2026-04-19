use serde_json::{json, Value};
use tracing::instrument;

use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

#[instrument(skip(params))]
pub fn cron_add(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;
    let expression = params
        .get("expression")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing expression".into()))?;
    Ok(json!({ "job_id": "cron-1", "name": name, "expression": expression }))
}

#[instrument(skip(params))]
pub fn cron_remove(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let job_id = params
        .get("job_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing job_id".into()))?;
    Ok(json!({ "job_id": job_id, "removed": true }))
}

#[instrument(skip(_params))]
pub fn cron_list(_ctx: &SyscallContext, _params: Value) -> SyscallResult {
    Ok(json!({ "jobs": [] }))
}
