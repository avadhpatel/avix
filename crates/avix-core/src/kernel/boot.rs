use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::error::AvixError;
use crate::kernel::proc::ProcHandler;
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::table::ProcessTable;
use crate::types::Pid;

/// Kernel boot phase 3: re-adopt orphaned agents from agents.yaml.
/// Any PID in agents.yaml that is not yet registered in the process table is re-adopted
/// as Running. PIDs already present in the table are skipped (idempotent).
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
        // Liveness check: consult the kernel process table, not the host OS.
        // Avix PIDs are virtual — they have no meaning as host OS process IDs.
        if process_table.get(Pid::new(record.pid)).await.is_some() {
            info!(pid = record.pid, name = %record.name, "agent already registered, skipping re-adopt");
            continue;
        }

        info!(pid = record.pid, name = %record.name, "re-adopting agent from agents.yaml");

        let entry = ProcessEntry {
            pid: Pid::new(record.pid),
            name: record.name.clone(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "kernel".to_string(),
            goal: record.goal.clone(),
            spawned_at: record.spawned_at,
            ..Default::default()
        };

        process_table.insert(entry).await;

        // TODO: Rewrite /proc/<pid>/ files
        // TODO: Send SIGSTART to resume IPC

        re_adopted += 1;
    }

    info!(re_adopted, "re-adopted agents from agents.yaml");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_agents_yaml(
        agents: Vec<crate::kernel::proc::AgentRecord>,
    ) -> crate::kernel::proc::AgentsYaml {
        crate::kernel::proc::AgentsYaml { agents }
    }

    fn make_record(pid: u32, name: &str) -> crate::kernel::proc::AgentRecord {
        crate::kernel::proc::AgentRecord {
            pid,
            name: name.to_string(),
            goal: "test goal".to_string(),
            session_id: "sess-1".to_string(),
            spawned_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn re_adopt_registers_agents_not_in_process_table() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());

        let yaml_str = serde_yaml::to_string(&make_agents_yaml(vec![
            make_record(10, "agent-a"),
            make_record(11, "agent-b"),
        ]))
        .unwrap();
        std::fs::write(&yaml_path, yaml_str).unwrap();

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        let entries = table.list_by_kind(ProcessKind::Agent).await;
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e.pid.as_u32() == 10 && e.name == "agent-a"));
        assert!(entries
            .iter()
            .any(|e| e.pid.as_u32() == 11 && e.name == "agent-b"));
        assert!(entries.iter().all(|e| e.status == ProcessStatus::Running));
    }

    #[tokio::test]
    async fn re_adopt_skips_pids_already_in_process_table() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());

        // Pre-register PID 20 so it's already in the table
        table
            .insert(ProcessEntry {
                pid: Pid::new(20),
                name: "already-running".to_string(),
                kind: ProcessKind::Agent,
                status: ProcessStatus::Running,
                goal: "original goal".to_string(),
                spawned_by_user: "kernel".to_string(),
                spawned_at: chrono::Utc::now(),
                ..Default::default()
            })
            .await;

        let yaml_str = serde_yaml::to_string(&make_agents_yaml(vec![
            make_record(20, "already-running"),
            make_record(21, "new-agent"),
        ]))
        .unwrap();
        std::fs::write(&yaml_path, yaml_str).unwrap();

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        // PID 20 was already there (not duplicated), PID 21 was added
        let entries = table.list_by_kind(ProcessKind::Agent).await;
        assert_eq!(entries.len(), 2);
        // PID 20 keeps its original entry (not replaced)
        let e20 = entries.iter().find(|e| e.pid.as_u32() == 20).unwrap();
        assert_eq!(e20.name, "already-running");
        assert!(entries.iter().any(|e| e.pid.as_u32() == 21));
    }

    #[tokio::test]
    async fn re_adopt_is_noop_when_agents_yaml_missing() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml"); // does not exist
        let table = Arc::new(ProcessTable::new());

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        assert!(table.list_by_kind(ProcessKind::Agent).await.is_empty());
    }
}
