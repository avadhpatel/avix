use serde_json::Value;

use crate::error::AvixError;

use super::super::service::{CallerContext, MemoryService};

/// Handle `memory/share-request`.
///
/// Validates that the caller holds `memory:share` (i.e. `memory/share-request` in
/// `granted_tools`) and that sharing is within the same owner namespace (v1 constraint).
/// Full HIL flow (SIGPAUSE / ApprovalToken / SIGRESUME) is wired in memory-gap-G when
/// the kernel HIL subsystem is available.
pub async fn handle(
    _svc: &MemoryService,
    params: Value,
    caller: &CallerContext,
) -> Result<Value, AvixError> {
    // 1. Capability check — memory:share is a privilege-level cap
    if !caller
        .granted_tools
        .contains(&"memory/share-request".to_string())
    {
        return Err(AvixError::CapabilityDenied(
            "memory:share not granted — agent manifest must set sharing.canRequest: true"
                .to_string(),
        ));
    }

    // 2. v1 constraint: cross-user sharing not supported
    // targetOwner must equal caller.owner (defaults to caller.owner)
    let target_owner = params["targetOwner"]
        .as_str()
        .unwrap_or(&caller.owner)
        .to_string();
    if target_owner != caller.owner {
        return Err(AvixError::CapabilityDenied(
            "cross-user memory sharing is not supported in v1 (crossUserEnabled: false)"
                .to_string(),
        ));
    }

    // 3. HIL flow deferred — kernel integration wired in memory-gap-G
    // Return pending stub; the real implementation calls kernel.mint_approval_token(),
    // kernel.write_hil_event(), and kernel.send_signal(SIGPAUSE).
    Err(AvixError::NotFound(
        "memory/share-request HIL flow not yet wired (memory-gap-G)".to_string(),
    ))
}
