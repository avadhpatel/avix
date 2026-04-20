use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn, instrument};
use uuid::Uuid;

use crate::agent_manifest::{AgentManifestSummary, ManifestScanner};
use crate::error::AvixError;
use crate::executor::{AgentExecutorFactory, SpawnParams};
use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
use crate::kernel::capability_resolver::CapabilityResolver;
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::ProcessTable;
use crate::router::ALWAYS_PRESENT;
use crate::session::{PersistentSessionStore, record::PidInvocationMeta};
use crate::syscall::SyscallRegistry;
use crate::tool_registry::ToolRegistry;
use crate::types::token::{CapabilityToken, IssuedTo};
use crate::types::Pid;

pub struct AgentManager {
    process_table: Arc<ProcessTable>,
    runtime_dir: PathBuf,
    master_key: Vec<u8>,
    executor_factory: Option<Arc<dyn AgentExecutorFactory>>,
    task_handles: Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>>,
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
    manifest_scanner: Option<Arc<ManifestScanner>>,
    active_invocations: Arc<Mutex<HashMap<u64, String>>>,
    active_sessions: Arc<Mutex<HashMap<u64, String>>>,
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,
}

impl AgentManager {
    pub fn new(
        process_table: Arc<ProcessTable>,
        runtime_dir: PathBuf,
        master_key: Vec<u8>,
        executor_factory: Option<Arc<dyn AgentExecutorFactory>>,
        invocation_store: Option<Arc<InvocationStore>>,
        session_store: Option<Arc<PersistentSessionStore>>,
        manifest_scanner: Option<Arc<ManifestScanner>>,
        active_invocations: Arc<Mutex<HashMap<u64, String>>>,
        active_sessions: Arc<Mutex<HashMap<u64, String>>>,
        tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,
    ) -> Self {
        Self {
            process_table,
            runtime_dir,
            master_key,
            executor_factory,
            task_handles: Arc::new(Mutex::new(HashMap::new())),
            invocation_store,
            session_store,
            manifest_scanner,
            active_invocations,
            active_sessions,
            tool_registry,
        }
    }

    #[instrument(skip(self, name, goal, session_id, atp_session_id, caller_identity))]
    pub async fn spawn(
        &self,
        name: &str,
        goal: &str,
        session_id: &str,
        atp_session_id: &str,
        caller_identity: &str,
        parent_pid: Option<u64>,
    ) -> Result<u64, AvixError> {
        info!(name, goal, session_id, atp_session_id, ?parent_pid, "spawning agent");

        let pid = Pid::generate().as_u64();
        debug!(pid, "allocated PID");

        let effective_session_id = if let Some(ppid) = parent_pid {
            let inherited = self.active_sessions.lock().await.get(&ppid).cloned();
            if let Some(sid) = inherited {
                if let Some(store) = &self.session_store {
                    if let Ok(Some(mut session)) = store.get(&Uuid::parse_str(&sid)?).await {
                        session.add_participant(name, true);
                        if let Err(e) = store.update(&session).await {
                            warn!(error = %e, "failed to update session with participant");
                        }
                    }
                }
                info!(session_id = %sid, parent_pid = ppid, "child inheriting parent session");
                sid
            } else {
                warn!(parent_pid = ppid, "parent not in active sessions, creating new session");
                self.resolve_session_from_id(name, goal, session_id, caller_identity, pid).await?
            }
        } else {
            self.resolve_session_from_id(name, goal, session_id, caller_identity, pid).await?
        };

        let entry = ProcessEntry {
            pid: Pid::from_u64(pid),
            name: name.to_string(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Pending,
            parent: parent_pid.map(Pid::from_u64),
            spawned_by_user: caller_identity.to_string(),
            goal: goal.to_string(),
            spawned_at: chrono::Utc::now(),
            ..Default::default()
        };

        self.process_table.insert(entry).await;
        debug!(pid, "inserted process entry");

        if let Some(store) = &self.session_store {
            if let Ok(uuid) = Uuid::parse_str(&effective_session_id) {
                if let Ok(Some(mut session)) = store.get(&uuid).await {
                    session.add_pid(pid);
                    if let Err(e) = store.update(&session).await {
                        warn!(pid, error = %e, "failed to add pid to session");
                    }
                }
            }
        }
        self.active_sessions.lock().await.insert(pid, effective_session_id.clone());

        let invocation_id = Uuid::new_v4().to_string();
        if let Some(store) = &self.invocation_store {
            let record = InvocationRecord::new(
                invocation_id.clone(),
                name.to_string(),
                caller_identity.to_string(),
                pid,
                goal.to_string(),
                effective_session_id.clone(),
            );
            if let Err(e) = store.create(&record).await {
                warn!(pid, invocation_id = %invocation_id, error = %e, "failed to create invocation");
            }
        }

        // Record per-PID metadata on the session so the session entry is self-describing.
        if let Some(sstore) = &self.session_store {
            if let Ok(uuid) = Uuid::parse_str(&effective_session_id) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    session.add_invocation_pid(PidInvocationMeta {
                        pid,
                        invocation_id: invocation_id.clone(),
                        agent_name: name.to_string(),
                        agent_version: String::new(),
                        spawned_at: chrono::Utc::now(),
                    });
                    if let Err(e) = sstore.update(&session).await {
                        warn!(pid, error = %e, "failed to record invocation pid meta on session");
                    } else {
                        debug!(pid, session_id = %effective_session_id, "recorded invocation pid meta on session");
                    }
                }
            }
        }

        self.active_invocations.lock().await.insert(pid, invocation_id.clone());
        debug!(pid, invocation_id = %invocation_id, "created invocation record");

        let issued_to = IssuedTo {
            pid,
            agent_name: name.to_string(),
            spawned_by: caller_identity.to_string(),
        };
        let granted_tools = self.resolve_granted_tools(name, caller_identity).await;
        debug!(pid, agent = name, tools = ?granted_tools, "resolved granted tools from manifest");
        let token = CapabilityToken::mint(
            granted_tools,
            Some(issued_to),
            3600,
            &self.master_key,
        );

        if let Some(factory) = &self.executor_factory {
            let spawn_params = SpawnParams {
                pid: Pid::from_u64(pid),
                agent_name: name.to_string(),
                goal: goal.to_string(),
                spawned_by: caller_identity.to_string(),
                session_id: effective_session_id.clone(),
                atp_session_id: atp_session_id.to_string(),
                token,
                system_prompt: None,
                selected_model: String::new(),
                denied_tools: vec![],
                context_limit: 0,
                runtime_dir: self.runtime_dir.clone(),
                invocation_id: invocation_id.clone(),
            };
            let abort_handle = factory.launch(spawn_params);
            self.task_handles.lock().await.insert(pid, abort_handle);
            info!(pid, "executor task launched");
        }

        self.process_table.set_status(Pid::from_u64(pid), ProcessStatus::Running).await?;
        info!(pid, name, "agent spawned successfully");

        Ok(pid)
    }

    /// Resolve the tool names to grant to a spawned agent.
    ///
    /// Loads the agent manifest from `ManifestScanner`, extracts
    /// `requestedCapabilities`, and maps them to concrete tool names via
    /// `CapabilityResolver`.  Falls back to `ALWAYS_PRESENT` only if the
    /// manifest or registry is unavailable.
    async fn resolve_granted_tools(&self, agent_name: &str, caller: &str) -> Vec<String> {
        let cap_groups: Vec<String> = if let Some(scanner) = &self.manifest_scanner {
            match scanner.get_manifest(agent_name, caller).await {
                Some(m) => m.spec.requested_capabilities,
                None => {
                    warn!(agent_name, "manifest not found; granting always-present tools only");
                    vec![]
                }
            }
        } else {
            warn!("manifest_scanner not wired; granting always-present tools only");
            vec![]
        };

        let syscall_reg = SyscallRegistry::new();
        let guard = self.tool_registry.lock().await;
        if let Some(tool_reg) = guard.as_ref() {
            let resolver = CapabilityResolver::new(tool_reg, &syscall_reg);
            resolver.resolve(&cap_groups).await
        } else {
            warn!(agent_name, "tool_registry not wired; granting always-present tools only");
            ALWAYS_PRESENT.iter().map(|s| s.to_string()).collect()
        }
    }

    #[instrument(skip(self))]
    pub async fn list(&self) -> Result<Vec<super::types::ActiveAgent>, AvixError> {
        debug!("listing active agents");

        let running = self.process_table.list_by_kind(ProcessKind::Agent).await;
        let mut active = Vec::new();
        for entry in running {
            let status = match entry.status {
                ProcessStatus::Running => "running",
                ProcessStatus::Paused => "paused",
                ProcessStatus::Waiting => "waiting",
                ProcessStatus::Stopped => "stopped",
                ProcessStatus::Crashed => "crashed",
                ProcessStatus::Pending => "pending",
            }.to_string();
            active.push(super::types::ActiveAgent {
                pid: entry.pid.as_u64(),
                name: entry.name,
                status,
                goal: entry.goal,
            });
        }
        Ok(active)
    }

    #[instrument(skip(self))]
    pub async fn abort_agent(&self, pid: u64) {
        info!(pid, "aborting agent");

        let mut handles = self.task_handles.lock().await;
        if let Some(handle) = handles.remove(&pid) {
            handle.abort();
            debug!(pid, "aborted executor task");
        } else {
            warn!(pid, "no executor task found for agent");
        }
        drop(handles);

        let _ = self.process_table.set_status(Pid::from_u64(pid), ProcessStatus::Stopped).await;
        debug!(pid, "set process status to Stopped");

        self.finalize_invocation(pid, InvocationStatus::Killed, Some("killed".into())).await;
    }

    #[instrument(skip(self))]
    pub async fn list_installed(&self, username: &str) -> Vec<AgentManifestSummary> {
        match &self.manifest_scanner {
            Some(scanner) => scanner.scan(username).await,
            None => vec![],
        }
    }

    #[instrument(skip_all)]
    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    #[instrument(skip_all)]
    pub fn task_handles(&self) -> &Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>> {
        &self.task_handles
    }

    #[instrument(skip_all)]
    pub fn active_invocations(&self) -> &Arc<Mutex<HashMap<u64, String>>> {
        &self.active_invocations
    }

    #[instrument(skip_all)]
    pub fn active_sessions(&self) -> &Arc<Mutex<HashMap<u64, String>>> {
        &self.active_sessions
    }

    pub fn invocation_store(&self) -> Option<&Arc<InvocationStore>> {
        self.invocation_store.as_ref()
    }

    pub fn session_store(&self) -> Option<&Arc<PersistentSessionStore>> {
        self.session_store.as_ref()
    }

    async fn finalize_invocation(
        &self,
        pid: u64,
        status: InvocationStatus,
        exit_reason: Option<String>,
    ) {
        let inv_id = self.active_invocations.lock().await.remove(&pid);
        let inv_id = match inv_id {
            Some(id) => id,
            None => return,
        };
        let store = match &self.invocation_store {
            Some(s) => s,
            None => return,
        };
        let (tokens, tool_calls) = match self.process_table.get(Pid::from_u64(pid)).await {
            Some(entry) => (entry.tokens_consumed, entry.tool_calls_total),
            None => (0, 0),
        };
        let _ = store.finalize(&inv_id, status, chrono::Utc::now(), tokens, tool_calls, exit_reason).await;
    }

    async fn resolve_session_from_id(
        &self,
        name: &str,
        goal: &str,
        session_id: &str,
        caller_identity: &str,
        owner_pid: u64,
    ) -> Result<String, AvixError> {
        if session_id.is_empty() {
            if let Some(store) = &self.session_store {
                let record = crate::session::SessionRecord::new(
                    Uuid::new_v4(),
                    caller_identity.to_string(),
                    name.to_string(),
                    name.to_string(),
                    goal.to_string(),
                    owner_pid,
                );
                let _ = store.create(&record).await;
                Ok(record.id.to_string())
            } else {
                Ok(Uuid::new_v4().to_string())
            }
        } else {
            if let Some(store) = &self.session_store {
                if let Ok(Some(mut session)) = store.get(&Uuid::parse_str(session_id)?).await {
                    session.add_participant(name, true);
                    let _ = store.update(&session).await;
                }
            }
            Ok(session_id.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingFactory {
        count: Arc<AtomicU32>,
    }

    impl AgentExecutorFactory for CountingFactory {
        fn launch(&self, _params: SpawnParams) -> tokio::task::AbortHandle {
            self.count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async {}).abort_handle()
        }
    }

    #[tokio::test]
    async fn spawn_with_factory_launches_executor_task() {
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let count = Arc::new(AtomicU32::new(0));
        let runtime_dir = PathBuf::from("/run/avix");
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let factory = Arc::new(CountingFactory {
            count: Arc::clone(&count),
        });
        let manager = AgentManager::new(
            table.clone(),
            runtime_dir,
            master_key,
            Some(factory),
            None,
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::new(Mutex::new(None)),
        );

        let pid1 = manager.spawn("agent-a", "goal-a", "sess-1", "", "kernel", None).await.unwrap();
        let pid2 = manager.spawn("agent-b", "goal-b", "sess-1", "", "kernel", None).await.unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(table.get(Pid::from_u64(pid1)).await.unwrap().status, ProcessStatus::Running);
        assert_eq!(table.get(Pid::from_u64(pid2)).await.unwrap().status, ProcessStatus::Running);

        manager.abort_agent(pid1).await;
        let handles = manager.task_handles.lock().await;
        assert!(!handles.contains_key(&pid1));
        assert!(handles.contains_key(&pid2));
    }

    #[tokio::test]
    async fn spawn_without_factory_still_registers_process() {
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let runtime_dir = PathBuf::from("/run/avix");
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table.clone(),
            runtime_dir,
            master_key,
            None,
            None,
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::new(Mutex::new(None)),
        );

        let pid = manager.spawn("agent", "goal", "sess", "", "kernel", None).await.unwrap();
        let entry = table.get(Pid::from_u64(pid)).await.unwrap();
        assert_eq!(entry.status, ProcessStatus::Running);
        assert!(manager.task_handles.lock().await.is_empty());
    }

    #[tokio::test]
    async fn list_returns_active_agents() {
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let runtime_dir = PathBuf::from("/run/avix");
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            runtime_dir,
            master_key,
            None,
            None,
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::new(Mutex::new(None)),
        );

        let pid1 = manager.spawn("agent1", "goal1", "sess-1", "", "kernel", None).await.unwrap();
        let pid2 = manager.spawn("agent2", "goal2", "sess-1", "", "kernel", None).await.unwrap();

        let active = manager.list().await.unwrap();
        assert_eq!(active.len(), 2);

        let a1 = active.iter().find(|a| a.pid == pid1).unwrap();
        assert_eq!(a1.name, "agent1");
        assert_eq!(a1.status, "running");

        let a2 = active.iter().find(|a| a.pid == pid2).unwrap();
        assert_eq!(a2.name, "agent2");
    }

    #[tokio::test]
    async fn spawn_without_parent_pid_creates_new_session() {
        let dir = TempDir::new().unwrap();
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let runtime_dir = PathBuf::from("/run/avix");
        let sstore = Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        );
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            runtime_dir,
            master_key,
            None,
            None,
            Some(Arc::clone(&sstore)),
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::new(Mutex::new(None)),
        );

        let pid = manager.spawn("agent-a", "goal", "", "", "alice", None).await.unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].pids.contains(&pid));
        assert_eq!(sessions[0].owner_pid, pid);
    }

    #[tokio::test]
    async fn spawn_with_parent_pid_inherits_session() {
        let dir = TempDir::new().unwrap();
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let runtime_dir = PathBuf::from("/run/avix");
        let sstore = Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        );
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            runtime_dir,
            master_key,
            None,
            None,
            Some(Arc::clone(&sstore)),
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::new(Mutex::new(None)),
        );

        let parent_pid = manager.spawn("parent-agent", "parent goal", "", "", "alice", None).await.unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        let parent_session_id = sessions[0].id;

        let child_pid = manager.spawn("child-agent", "child goal", "", "", "alice", Some(parent_pid)).await.unwrap();

        let session = sstore.get(&parent_session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&parent_pid));
        assert!(session.pids.contains(&child_pid));
    }

    // ── Resolver tests ────────────────────────────────────────────────────────

    use crate::agent_manifest::ManifestScanner;
    use crate::memfs::{VfsPath, VfsRouter};
    use crate::tool_registry::{ToolRegistry, entry::ToolEntry};
    use crate::types::tool::{ToolName, ToolState, ToolVisibility};
    use crate::router::ALWAYS_PRESENT;

    /// Factory that captures the `granted_tools` from the last `SpawnParams`.
    struct CapturingFactory {
        captured_tools: Arc<Mutex<Vec<String>>>,
    }

    impl AgentExecutorFactory for CapturingFactory {
        fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle {
            let tools = params.token.granted_tools.clone();
            let captured = Arc::clone(&self.captured_tools);
            tokio::spawn(async move {
                *captured.lock().await = tools;
            })
            .abort_handle()
        }
    }

    const EXPLORER_YAML: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: explorer
  version: 0.1.0
  description: Explorer
  author: test
spec:
  requestedCapabilities:
    - fs:*
    - llm:*
  entrypoint:
    type: llm-loop
"#;

    fn make_tool(name: &str) -> ToolEntry {
        ToolEntry::new(
            ToolName::parse(name).unwrap(),
            "test".to_string(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::Value::Null,
        )
    }

    async fn make_vfs_with_manifest(path: &str, yaml: &str) -> Arc<VfsRouter> {
        let vfs = Arc::new(VfsRouter::new());
        let p = VfsPath::parse(path).unwrap();
        vfs.write(&p, yaml.as_bytes().to_vec()).await.unwrap();
        vfs
    }

    #[tokio::test]
    async fn spawn_resolves_tools_from_manifest() {
        let vfs = make_vfs_with_manifest("/bin/explorer@0.1.0/manifest.yaml", EXPLORER_YAML).await;
        let scanner = Arc::new(ManifestScanner::new(Arc::clone(&vfs)));

        let tool_reg = Arc::new(ToolRegistry::new());
        tool_reg.add("test", vec![make_tool("fs/read"), make_tool("llm/complete")]).await.unwrap();
        let tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>> =
            Arc::new(Mutex::new(Some(Arc::clone(&tool_reg))));

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let factory = Arc::new(CapturingFactory { captured_tools: Arc::clone(&captured) });

        let table = Arc::new(ProcessTable::new());
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            PathBuf::from("/run/avix"),
            b"test-master-key-32-bytes-padded!".to_vec(),
            Some(factory),
            None,
            None,
            Some(scanner),
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::clone(&tool_registry),
        );

        manager.spawn("explorer", "goal", "sess", "", "alice", None).await.unwrap();
        // Give the spawned task a tick to run
        tokio::task::yield_now().await;

        let tools = captured.lock().await;
        assert!(tools.contains(&"fs/read".to_string()), "fs/read missing from token");
        assert!(tools.contains(&"llm/complete".to_string()), "llm/complete missing from token");
        for ap in ALWAYS_PRESENT {
            assert!(tools.contains(&ap.to_string()), "always-present {ap} missing from token");
        }
    }

    #[tokio::test]
    async fn spawn_falls_back_when_manifest_missing() {
        // Scanner has no manifest for "ghost-agent"
        let vfs = Arc::new(VfsRouter::new());
        let scanner = Arc::new(ManifestScanner::new(vfs));
        let tool_reg = Arc::new(ToolRegistry::new());
        tool_reg.add("test", vec![make_tool("fs/read")]).await.unwrap();
        let tool_registry = Arc::new(Mutex::new(Some(Arc::clone(&tool_reg))));

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let factory = Arc::new(CapturingFactory { captured_tools: Arc::clone(&captured) });

        let table = Arc::new(ProcessTable::new());
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            PathBuf::from("/run/avix"),
            b"test-master-key-32-bytes-padded!".to_vec(),
            Some(factory),
            None,
            None,
            Some(scanner),
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            tool_registry,
        );

        manager.spawn("ghost-agent", "goal", "sess", "", "alice", None).await.unwrap();
        tokio::task::yield_now().await;

        let tools = captured.lock().await;
        // Only ALWAYS_PRESENT — no fs/read because no manifest matched
        assert!(!tools.contains(&"fs/read".to_string()));
        for ap in ALWAYS_PRESENT {
            assert!(tools.contains(&ap.to_string()));
        }
    }

    #[tokio::test]
    async fn spawn_falls_back_when_registry_not_wired() {
        let vfs = make_vfs_with_manifest("/bin/explorer@0.1.0/manifest.yaml", EXPLORER_YAML).await;
        let scanner = Arc::new(ManifestScanner::new(vfs));
        // tool_registry is None — not wired
        let tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>> =
            Arc::new(Mutex::new(None));

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let factory = Arc::new(CapturingFactory { captured_tools: Arc::clone(&captured) });

        let table = Arc::new(ProcessTable::new());
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));

        let manager = AgentManager::new(
            table,
            PathBuf::from("/run/avix"),
            b"test-master-key-32-bytes-padded!".to_vec(),
            Some(factory),
            None,
            None,
            Some(scanner),
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            tool_registry,
        );

        manager.spawn("explorer", "goal", "sess", "", "alice", None).await.unwrap();
        tokio::task::yield_now().await;

        let tools = captured.lock().await;
        // Only ALWAYS_PRESENT — registry not wired
        for ap in ALWAYS_PRESENT {
            assert!(tools.contains(&ap.to_string()));
        }
        assert!(!tools.contains(&"fs/read".to_string()));
    }
}
