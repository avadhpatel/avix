/// Capability enforcement for tool dispatch (ADR-01).
///
/// Checks that the calling process has the requested tool in its `CapabilityToken`.
/// Always-present tools bypass this check — they are available regardless of grants.
use crate::error::AvixError;
use crate::process::ProcessTable;
use crate::types::Pid;
use std::sync::Arc;

/// Tools that are always available to every agent, regardless of capability grants.
/// Defined in ADR-04 and §13 of CLAUDE.md.
pub const ALWAYS_PRESENT: &[&str] = &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"];

/// Check that `caller_pid`'s process entry grants the named tool.
///
/// Returns `Ok(())` if the tool is always-present or if the process's
/// `granted_tools` list contains the tool name.
///
/// Returns `Err(AvixError::CapabilityDenied)` if the process is not found
/// or if the tool is not in the granted list.
pub async fn check_capability(
    tool: &str,
    caller_pid: Pid,
    process_table: &Arc<ProcessTable>,
) -> Result<(), AvixError> {
    // Always-present tools bypass the capability check.
    if ALWAYS_PRESENT.contains(&tool) {
        return Ok(());
    }

    let entry = process_table.get(caller_pid).await.ok_or_else(|| {
        AvixError::CapabilityDenied(format!("no process entry for pid {caller_pid}"))
    })?;

    if entry.granted_tools.iter().any(|t| t == tool) {
        Ok(())
    } else {
        Err(AvixError::CapabilityDenied(format!(
            "tool '{tool}' not granted to pid {caller_pid}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{ProcessEntry, ProcessKind, ProcessStatus};
    use crate::types::Pid;
    use std::sync::Arc;

    async fn table_with_entry(tools: Vec<String>) -> Arc<ProcessTable> {
        let table = Arc::new(ProcessTable::new());
        table
            .insert(ProcessEntry {
                pid: Pid::new(10),
                name: "test-agent".into(),
                kind: ProcessKind::Agent,
                status: ProcessStatus::Running,
                spawned_by_user: "alice".into(),
                granted_tools: tools,
                ..Default::default()
            })
            .await;
        table
    }

    #[tokio::test]
    async fn granted_tool_is_allowed() {
        let table = table_with_entry(vec!["fs/read".into()]).await;
        check_capability("fs/read", Pid::new(10), &table)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn ungranted_tool_is_denied() {
        let table = table_with_entry(vec!["fs/read".into()]).await;
        let result = check_capability("fs/write", Pid::new(10), &table).await;
        assert!(matches!(result, Err(AvixError::CapabilityDenied(_))));
    }

    #[tokio::test]
    async fn always_present_tools_bypass_check() {
        // Process with no granted tools at all.
        let table = table_with_entry(vec![]).await;
        for tool in ALWAYS_PRESENT {
            check_capability(tool, Pid::new(10), &table).await.unwrap();
        }
    }

    #[tokio::test]
    async fn missing_process_returns_denied() {
        let table = Arc::new(ProcessTable::new());
        let result = check_capability("fs/read", Pid::new(99), &table).await;
        assert!(matches!(result, Err(AvixError::CapabilityDenied(_))));
    }
}
