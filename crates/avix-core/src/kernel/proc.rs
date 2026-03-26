use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::error::AvixError;
use crate::process::table::ProcessTable;
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::types::Pid;

/// Persistent record of a spawned agent, stored in /etc/avix/agents.yaml.
/// Used for daemon restart to re-adopt running agents.
/// Links: docs/architecture/08-llm-service.md#configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub pid: u32,
    pub name: String,
    pub goal: String,
    pub session_id: String,
    pub spawned_at: chrono::DateTime<chrono::Utc>,
}

/// The root-owned agents.yaml file containing all spawned agents.
/// Links: docs/spec/runtime-exec-tool-exposure.md#category-2-registration-lifecycle
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsYaml {
    pub agents: Vec<AgentRecord>,
}

/// Active agent summary returned by proc/list.
/// Links: docs/spec/avix-terminal-protocol.md#6-2-proc-agent-lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAgent {
    pub pid: u32,
    pub name: String,
    pub status: String,
    pub goal: String,
}

/// Kernel proc domain handler.
/// Provides spawn, list, and persistence operations.
/// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
pub struct ProcHandler {
    process_table: Arc<ProcessTable>,
    agents_yaml_path: PathBuf,
}

impl ProcHandler {
    /// Create a new proc handler with the given process table and agents.yaml path.
    /// The path should be /etc/avix/agents.yaml (root-owned).
    pub fn new(process_table: Arc<ProcessTable>, agents_yaml_path: PathBuf) -> Self {
        Self {
            process_table,
            agents_yaml_path,
        }
    }

    /// Spawn a new agent: allocate PID, mint CapToken, write /proc/ files, persist to agents.yaml, fork/exec RuntimeExecutor.
    /// Returns the allocated PID.
    /// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
    pub async fn spawn(&self, name: &str, goal: &str, session_id: &str, caller_identity: &str) -> Result<u32, AvixError> {
        info!(name, goal, session_id, "spawning agent");

        // Allocate PID (simple increment for now)
        let pid = self.allocate_pid().await?;
        info!(pid, "allocated PID");

        // Create process entry
        let entry = ProcessEntry {
            pid: Pid::new(pid),
            name: name.to_string(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Pending,
            parent: None, // kernel spawn
            spawned_by_user: "kernel".to_string(), // TODO: get from context
            goal: goal.to_string(),
            spawned_at: chrono::Utc::now(),
            ..Default::default()
        };

        // Insert into process table
        self.process_table.insert(entry).await;

        // Persist to agents.yaml
        self.persist_agent_record(pid, name, goal, session_id).await?;
        info!(pid, "persisted agent record to agents.yaml");

        // Write /proc/<pid>/status.yaml and resolved.yaml
        // TODO: Implement init_proc_files here or in RuntimeExecutor

        // Fork/exec avix-re (RuntimeExecutor)
        use std::process::Stdio;
        use tokio::process::Command;

        let token_json = serde_json::to_string(&token)?;
        let mut cmd = Command::new("./target/debug/avix-re");
        cmd.env("AVIX_PID", pid.to_string())
            .env("AVIX_GOAL", goal)
            .env("AVIX_TOKEN", token_json)
            .env("AVIX_SESSION_ID", session_id)
            .env("AVIX_AGENT_NAME", name)
            .env("AVIX_SPAWNED_BY", "kernel") // TODO: get from context
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Spawn the process
        let child = cmd.spawn().map_err(|e| AvixError::Io(format!("Failed to spawn avix-re: {}", e)))?;

        // TODO: Store the child handle to manage it (kill, etc.)
        // For now, detach it
        tokio::spawn(async move {
            let _ = child.wait().await;
            // TODO: On exit, send agent.exit event, remove from agents.yaml
        });

        // Mark as running
        self.process_table.set_status(Pid::new(pid), ProcessStatus::Running).await?;

        Ok(pid)
    }

    /// List all active agents: read agents.yaml + scan running PIDs → ActiveAgent vec.
    /// Emits AgentStatus events via the event bus.
    /// Links: docs/spec/avix-terminal-protocol.md#6-2-proc-agent-lifecycle
    pub async fn list(&self) -> Result<Vec<ActiveAgent>, AvixError> {
        debug!("listing active agents");

        // Read persisted agents from yaml
        let _persisted = self.load_agents_yaml().await.unwrap_or_default();

        // Get running PIDs from process table
        let running = self.process_table.list_by_kind(ProcessKind::Agent).await;

        // Build active agents list
        let mut active = Vec::new();
        for entry in running {
            let pid_u32 = entry.pid.as_u32();
            let status = match entry.status {
                ProcessStatus::Running => "running",
                ProcessStatus::Paused => "paused",
                ProcessStatus::Waiting => "waiting",
                ProcessStatus::Stopped => "stopped",
                ProcessStatus::Crashed => "crashed",
                ProcessStatus::Pending => "pending",
            }.to_string();

            active.push(ActiveAgent {
                pid: pid_u32,
                name: entry.name,
                status,
                goal: entry.goal,
            });

            // TODO: Emit AgentStatus event
        }

        info!(count = active.len(), "listed active agents");
        Ok(active)
    }

    /// Allocate a new unique PID.
    async fn allocate_pid(&self) -> Result<u32, AvixError> {
        // Simple allocation: find max PID + 1
        let entries = self.process_table.list_all().await;
        let max_pid = entries.iter().map(|e| e.pid.as_u32()).max().unwrap_or(0);
        Ok(max_pid + 1)
    }

    /// Persist agent record to agents.yaml (atomic write).
    /// Links: docs/architecture/08-llm-service.md#configuration
    async fn persist_agent_record(&self, pid: u32, name: &str, goal: &str, session_id: &str) -> Result<(), AvixError> {
        let mut agents = self.load_agents_yaml().await.unwrap_or_default();

        let record = AgentRecord {
            pid,
            name: name.to_string(),
            goal: goal.to_string(),
            session_id: session_id.to_string(),
            spawned_at: chrono::Utc::now(),
        };

        // Add or update
        if let Some(existing) = agents.agents.iter_mut().find(|a| a.pid == pid) {
            *existing = record;
        } else {
            agents.agents.push(record);
        }

        self.save_agents_yaml(&agents).await?;
        Ok(())
    }

    /// Load agents.yaml, return default if not exists.
    pub async fn load_agents_yaml(&self) -> Result<AgentsYaml, AvixError> {
        if !self.agents_yaml_path.exists() {
            return Ok(AgentsYaml { agents: Vec::new() });
        }

        let yaml = fs::read_to_string(&self.agents_yaml_path)
            .map_err(|e| AvixError::Io(e.to_string()))?;
        serde_yaml::from_str(&yaml)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Save agents.yaml atomically.
    async fn save_agents_yaml(&self, agents: &AgentsYaml) -> Result<(), AvixError> {
        let yaml = serde_yaml::to_string(agents)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let tmp_path = self.agents_yaml_path.with_extension("tmp");
        fs::write(&tmp_path, &yaml)
            .map_err(|e| AvixError::Io(e.to_string()))?;
        fs::rename(&tmp_path, &self.agents_yaml_path)
            .map_err(|e| AvixError::Io(e.to_string()))?;
        Ok(())
    }

    /// Remove agent record from agents.yaml (on exit).
    pub async fn remove_agent_record(&self, pid: u32) -> Result<(), AvixError> {
        let mut agents = self.load_agents_yaml().await.unwrap_or_default();
        agents.agents.retain(|a| a.pid != pid);
        self.save_agents_yaml(&agents).await?;
        info!(pid, "removed agent record from agents.yaml");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::Arc;

    #[tokio::test]
    async fn spawn_creates_process_entry_and_persists() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let handler = ProcHandler::new(table.clone(), yaml_path.clone());

        let pid = handler.spawn("test-agent", "test goal", "sess-1").await.unwrap();

        // Check process table
        let entry = table.get(Pid::new(pid)).await.unwrap();
        assert_eq!(entry.name, "test-agent");
        assert_eq!(entry.goal, "test goal");
        assert_eq!(entry.status, ProcessStatus::Running);

        // Check yaml
        let yaml: AgentsYaml = serde_yaml::from_str(&fs::read_to_string(&yaml_path).unwrap()).unwrap();
        assert_eq!(yaml.agents.len(), 1);
        assert_eq!(yaml.agents[0].pid, pid);
        assert_eq!(yaml.agents[0].name, "test-agent");
        assert_eq!(yaml.agents[0].goal, "test goal");
        assert_eq!(yaml.agents[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn list_returns_active_agents() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let handler = ProcHandler::new(table.clone(), yaml_path);

        // Spawn two agents
        let pid1 = handler.spawn("agent1", "goal1", "sess-1").await.unwrap();
        let pid2 = handler.spawn("agent2", "goal2", "sess-1").await.unwrap();

        let active = handler.list().await.unwrap();
        assert_eq!(active.len(), 2);

        let a1 = active.iter().find(|a| a.pid == pid1).unwrap();
        assert_eq!(a1.name, "agent1");
        assert_eq!(a1.goal, "goal1");
        assert_eq!(a1.status, "running");

        let a2 = active.iter().find(|a| a.pid == pid2).unwrap();
        assert_eq!(a2.name, "agent2");
        assert_eq!(a2.goal, "goal2");
        assert_eq!(a2.status, "running");
    }

    #[tokio::test]
    async fn remove_agent_record_cleans_up_yaml() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let handler = ProcHandler::new(table, yaml_path.clone());

        let pid = handler.spawn("test", "goal", "sess").await.unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 1);

        handler.remove_agent_record(pid).await.unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 0);
    }
}