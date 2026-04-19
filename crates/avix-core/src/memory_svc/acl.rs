use crate::error::AvixError;

use super::service::CallerContext;

use tracing::instrument;

/// Validates that the caller may write to their own memory namespace.
///
/// Agents may only write to their own agent_name namespace — cross-agent
/// writes are blocked by ACL. All writes go through memory.svc, never
/// directly via `fs/write`.
#[instrument]
pub fn check_write_namespace(caller: &CallerContext, target_agent: &str) -> Result<(), AvixError> {
    if caller.agent_name != target_agent {
        return Err(AvixError::CapabilityDenied(format!(
            "agent '{}' may not write to '{}' memory namespace",
            caller.agent_name, target_agent
        )));
    }
    Ok(())
}
