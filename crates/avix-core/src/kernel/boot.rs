use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tracing::{info, warn};

use crate::error::AvixError;
use crate::kernel::proc::ProcHandler;
use crate::process::table::ProcessTable;
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::types::Pid;

/// Kernel boot phase 3: re-adopt orphaned agents from agents.yaml.
/// Checks which persisted agents are still alive, rewrites /proc/ files, sends SIGSTART.
/// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
pub async fn phase3_re_adopt(
    process_table: Arc<ProcessTable>,
    agents_yaml_path: PathBuf,
    master_key: Vec<u8>,
) -> Result<(), AvixError> {
    info!("phase 3: re-adopting orphaned agents");

    let handler = ProcHandler::new(process_table.clone(), agents_yaml_path, master_key);

    // Load persisted agents
    let agents_yaml = match handler.load_agents_yaml().await {
        Ok(yaml) => yaml,
        Err(e) => {
            warn!("failed to load agents.yaml: {}", e);
            return Ok(()); // Not a fatal error
        }
    };

    let mut re_adopted = 0;
    for record in &agents_yaml.agents {
        if is_pid_alive(record.pid).await {
            info!(pid = record.pid, name = %record.name, "re-adopting alive agent");

            // Create process entry
            let entry = ProcessEntry {
                pid: Pid::new(record.pid),
                name: record.name.clone(),
                kind: ProcessKind::Agent,
                status: ProcessStatus::Running, // Assume running since alive
                parent: None,
                spawned_by_user: "kernel".to_string(), // TODO: store in yaml?
                goal: record.goal.clone(),
                spawned_at: record.spawned_at,
                ..Default::default()
            };

            // Insert into process table
            process_table.insert(entry).await;

            // TODO: Rewrite /proc/<pid>/ files
            // TODO: Send SIGSTART to resume IPC

            re_adopted += 1;
        } else {
            warn!(pid = record.pid, name = %record.name, "agent not alive, skipping re-adopt");
        }
    }

    info!(re_adopted, "re-adopted agents from agents.yaml");
    Ok(())
}

/// Check if a PID is alive by sending signal 0.
/// Links: docs/architecture/08-llm-service.md#health-checks
async fn is_pid_alive(pid: u32) -> bool {
    // Use `kill -0` to check if process exists without sending a signal
    match Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .await
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::Arc;

    #[tokio::test]
    async fn re_adopt_recreates_process_entries_for_alive_pids() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());

        // Use the current process PID — guaranteed alive and signallable by this process
        let alive_pid = std::process::id();

        // Create a dummy agents.yaml with one agent
        let agents = crate::kernel::proc::AgentsYaml {
            agents: vec![crate::kernel::proc::AgentRecord {
                pid: alive_pid,
                name: "test-agent".to_string(),
                goal: "test goal".to_string(),
                session_id: "sess-1".to_string(),
                spawned_at: chrono::Utc::now(),
            }],
        };
        let yaml_str = serde_yaml::to_string(&agents).unwrap();
        std::fs::write(&yaml_path, yaml_str).unwrap();

        // Run re-adopt (empty key is fine for tests — no tokens are actually verified)
        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32]).await.unwrap();

        // Check process table
        let entries = table.list_by_kind(ProcessKind::Agent).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].pid.as_u32(), alive_pid);
        assert_eq!(entries[0].name, "test-agent");
        assert_eq!(entries[0].goal, "test goal");
        assert_eq!(entries[0].status, ProcessStatus::Running);
    }

    #[tokio::test]
    async fn is_pid_alive_checks_process_existence() {
        // Test with our own PID (should be alive)
        let our_pid = std::process::id();
        assert!(is_pid_alive(our_pid).await);

        // Test with a high PID (unlikely to exist)
        assert!(!is_pid_alive(999999).await);
    }
}