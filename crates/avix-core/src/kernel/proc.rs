use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::error::AvixError;
use crate::executor::{AgentExecutorFactory, SpawnParams};
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::table::ProcessTable;
use crate::types::token::{CapabilityToken, IssuedTo};
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
    master_key: Vec<u8>,
    runtime_dir: PathBuf,
    executor_factory: Option<Arc<dyn AgentExecutorFactory>>,
    /// Abort handles for running executor tasks, keyed by Avix PID.
    task_handles: Arc<Mutex<HashMap<u32, tokio::task::AbortHandle>>>,
}

impl ProcHandler {
    /// Create a new proc handler. No executor factory — spawn() allocates a PID
    /// and updates the process table but does not launch an executor task.
    /// Used in tests and contexts where executor launch is not needed.
    pub fn new(
        process_table: Arc<ProcessTable>,
        agents_yaml_path: PathBuf,
        master_key: Vec<u8>,
    ) -> Self {
        Self {
            process_table,
            agents_yaml_path,
            master_key,
            runtime_dir: PathBuf::from("/run/avix"),
            executor_factory: None,
            task_handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a proc handler with an executor factory. `spawn()` will launch a
    /// background `RuntimeExecutor` tokio task for each agent via the factory.
    pub fn new_with_factory(
        process_table: Arc<ProcessTable>,
        agents_yaml_path: PathBuf,
        master_key: Vec<u8>,
        runtime_dir: PathBuf,
        factory: Arc<dyn AgentExecutorFactory>,
    ) -> Self {
        Self {
            process_table,
            agents_yaml_path,
            master_key,
            runtime_dir,
            executor_factory: Some(factory),
            task_handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Expose the process table for use by other kernel subsystems (e.g. ipc_server).
    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    /// Abort the background executor task for the given PID, if one is running.
    /// Called by the IPC kill handler so the tokio task is forcibly stopped.
    pub async fn abort_agent(&self, pid: u32) {
        let mut handles = self.task_handles.lock().await;
        if let Some(handle) = handles.remove(&pid) {
            handle.abort();
            info!(pid, "aborted executor task for killed agent");
        } else {
            warn!(
                pid,
                "no executor task found for agent (may have exited already)"
            );
        }
    }

    /// Spawn a new agent: allocate PID, mint CapToken, write /proc/ files, persist to agents.yaml, fork/exec RuntimeExecutor.
    /// Returns the allocated PID.
    /// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
    pub async fn spawn(
        &self,
        name: &str,
        goal: &str,
        session_id: &str,
        caller_identity: &str,
    ) -> Result<u32, AvixError> {
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
            parent: None,                          // kernel spawn
            spawned_by_user: "kernel".to_string(), // TODO: get from context
            goal: goal.to_string(),
            spawned_at: chrono::Utc::now(),
            ..Default::default()
        };

        // Insert into process table
        self.process_table.insert(entry).await;

        // Persist to agents.yaml
        self.persist_agent_record(pid, name, goal, session_id)
            .await?;
        info!(pid, "persisted agent record to agents.yaml");

        // Write /proc/<pid>/status.yaml and resolved.yaml
        // TODO: Implement init_proc_files here

        // Mint capability token for the agent
        let issued_to = IssuedTo {
            pid,
            agent_name: name.to_string(),
            spawned_by: caller_identity.to_string(),
        };
        let token = CapabilityToken::mint(
            vec![
                "fs/read".to_string(),
                "fs/write".to_string(),
                "agent/spawn".to_string(),
                "llm/complete".to_string(),
            ],
            Some(issued_to),
            3600,
            &self.master_key,
        );

        // Launch RuntimeExecutor as a background tokio task via the factory.
        // If no factory is configured (e.g. tests, or a kernel that manages
        // agents externally), skip launch and leave the status as Running so
        // callers can still track the PID through the process table.
        if let Some(factory) = &self.executor_factory {
            let spawn_params = SpawnParams {
                pid: Pid::new(pid),
                agent_name: name.to_string(),
                goal: goal.to_string(),
                spawned_by: caller_identity.to_string(),
                session_id: session_id.to_string(),
                token,
                system_prompt: None,
                selected_model: String::new(), // factory resolves via llm.svc
                denied_tools: vec![],
                context_limit: 0,
                runtime_dir: self.runtime_dir.clone(),
            };
            let abort_handle = factory.launch(spawn_params);
            self.task_handles.lock().await.insert(pid, abort_handle);
            info!(pid, "executor task launched");
        }

        // Mark as running
        self.process_table
            .set_status(Pid::new(pid), ProcessStatus::Running)
            .await?;

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
            }
            .to_string();

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
    /// PID 1 is reserved for the kernel agent; user agents start from 2.
    async fn allocate_pid(&self) -> Result<u32, AvixError> {
        let entries = self.process_table.list_all().await;
        // unwrap_or(1) ensures the first allocated PID is 2 even when the
        // process table is empty (kernel PID 1 is not yet inserted).
        let max_pid = entries.iter().map(|e| e.pid.as_u32()).max().unwrap_or(1);
        Ok(max_pid + 1)
    }

    /// Persist agent record to agents.yaml (atomic write).
    /// Links: docs/architecture/08-llm-service.md#configuration
    async fn persist_agent_record(
        &self,
        pid: u32,
        name: &str,
        goal: &str,
        session_id: &str,
    ) -> Result<(), AvixError> {
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

        let yaml =
            fs::read_to_string(&self.agents_yaml_path).map_err(|e| AvixError::Io(e.to_string()))?;
        serde_yaml::from_str(&yaml).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Save agents.yaml atomically.
    async fn save_agents_yaml(&self, agents: &AgentsYaml) -> Result<(), AvixError> {
        let yaml =
            serde_yaml::to_string(agents).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let tmp_path = self.agents_yaml_path.with_extension("tmp");
        fs::write(&tmp_path, &yaml).map_err(|e| AvixError::Io(e.to_string()))?;
        fs::rename(&tmp_path, &self.agents_yaml_path).map_err(|e| AvixError::Io(e.to_string()))?;
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
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Minimal factory that records how many times `launch` was called.
    struct CountingFactory {
        count: Arc<AtomicU32>,
    }

    impl AgentExecutorFactory for CountingFactory {
        fn launch(&self, _params: SpawnParams) -> tokio::task::AbortHandle {
            self.count.fetch_add(1, Ordering::SeqCst);
            // Spawn a no-op task so we have a real abort handle.
            tokio::spawn(async {}).abort_handle()
        }
    }

    #[tokio::test]
    async fn spawn_with_factory_launches_executor_task() {
        let dir = TempDir::new().unwrap();
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let count = Arc::new(AtomicU32::new(0));

        let factory = Arc::new(CountingFactory {
            count: Arc::clone(&count),
        });
        let handler = ProcHandler::new_with_factory(
            table.clone(),
            dir.path().join("agents.yaml"),
            master_key,
            dir.path().join("run/avix"),
            factory,
        );

        let pid1 = handler
            .spawn("agent-a", "goal-a", "sess-1", "kernel")
            .await
            .unwrap();
        let pid2 = handler
            .spawn("agent-b", "goal-b", "sess-1", "kernel")
            .await
            .unwrap();

        // Factory should have been called once per spawn
        assert_eq!(count.load(Ordering::SeqCst), 2);

        // Both pids registered and running
        assert_eq!(
            table.get(Pid::new(pid1)).await.unwrap().status,
            ProcessStatus::Running
        );
        assert_eq!(
            table.get(Pid::new(pid2)).await.unwrap().status,
            ProcessStatus::Running
        );

        // Abort handles stored — abort_agent should remove them
        handler.abort_agent(pid1).await;
        {
            let handles = handler.task_handles.lock().await;
            assert!(
                !handles.contains_key(&pid1),
                "handle for pid1 should be gone after abort"
            );
            assert!(
                handles.contains_key(&pid2),
                "handle for pid2 should still be present"
            );
        }
    }

    #[tokio::test]
    async fn spawn_without_factory_still_registers_process() {
        let dir = TempDir::new().unwrap();
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table.clone(), dir.path().join("agents.yaml"), master_key);

        let pid = handler
            .spawn("agent", "goal", "sess", "kernel")
            .await
            .unwrap();
        let entry = table.get(Pid::new(pid)).await.unwrap();
        assert_eq!(entry.status, ProcessStatus::Running);
        // No task handles stored
        assert!(handler.task_handles.lock().await.is_empty());
    }

    #[tokio::test]
    async fn spawn_creates_process_entry_and_persists() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table.clone(), yaml_path.clone(), master_key);

        let pid = handler
            .spawn("test_agent", "test_goal", "sess-1", "kernel")
            .await
            .unwrap();

        // Check process table
        let entry = table.get(Pid::new(pid)).await.unwrap();
        assert_eq!(entry.name, "test_agent");
        assert_eq!(entry.goal, "test_goal");
        assert_eq!(entry.status, ProcessStatus::Running);

        // Check yaml
        let yaml: AgentsYaml =
            serde_yaml::from_str(&fs::read_to_string(&yaml_path).unwrap()).unwrap();
        assert_eq!(yaml.agents.len(), 1);
        assert_eq!(yaml.agents[0].pid, pid);
        assert_eq!(yaml.agents[0].name, "test_agent");
        assert_eq!(yaml.agents[0].goal, "test_goal");
        assert_eq!(yaml.agents[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn list_returns_active_agents() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table.clone(), yaml_path, master_key);

        // Spawn two agents
        let pid1 = handler
            .spawn("agent1", "goal1", "sess-1", "kernel")
            .await
            .unwrap();
        let pid2 = handler
            .spawn("agent2", "goal2", "sess-1", "kernel")
            .await
            .unwrap();

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
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table, yaml_path.clone(), master_key);

        let pid = handler
            .spawn("test", "goal", "sess", "kernel")
            .await
            .unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 1);

        handler.remove_agent_record(pid).await.unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 0);
    }
}
