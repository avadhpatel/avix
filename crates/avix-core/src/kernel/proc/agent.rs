use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::agent_manifest::{AgentManifestSummary, ManifestScanner};
use crate::error::AvixError;
use crate::executor::{AgentExecutorFactory, SpawnParams};
use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::ProcessTable;
use crate::session::PersistentSessionStore;
use crate::types::token::{CapabilityToken, IssuedTo};
use crate::types::Pid;

use super::signals::SignalHandler;

pub struct AgentManager {
    process_table: Arc<ProcessTable>,
    runtime_dir: PathBuf,
    master_key: Vec<u8>,
    executor_factory: Option<Arc<dyn AgentExecutorFactory>>,
    task_handles: Arc<Mutex<HashMap<u32, tokio::task::AbortHandle>>>,
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
    manifest_scanner: Option<Arc<ManifestScanner>>,
    active_invocations: Arc<Mutex<HashMap<u32, String>>>,
    active_sessions: Arc<Mutex<HashMap<u32, String>>>,
    signal_handler: Arc<SignalHandler>,
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
        active_invocations: Arc<Mutex<HashMap<u32, String>>>,
        active_sessions: Arc<Mutex<HashMap<u32, String>>>,
        signal_handler: Arc<SignalHandler>,
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
            signal_handler,
        }
    }

    pub async fn spawn(
        &self,
        name: &str,
        goal: &str,
        session_id: &str,
        caller_identity: &str,
        parent_pid: Option<u32>,
    ) -> Result<u32, AvixError> {
        info!(name, goal, session_id, ?parent_pid, "spawning agent");

        let pid = self.allocate_pid().await?;
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
            pid: Pid::new(pid),
            name: name.to_string(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Pending,
            parent: parent_pid.map(Pid::new),
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
        self.active_invocations.lock().await.insert(pid, invocation_id.clone());
        debug!(pid, invocation_id = %invocation_id, "created invocation record");

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

        if let Some(factory) = &self.executor_factory {
            let spawn_params = SpawnParams {
                pid: Pid::new(pid),
                agent_name: name.to_string(),
                goal: goal.to_string(),
                spawned_by: caller_identity.to_string(),
                session_id: effective_session_id.clone(),
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

        self.process_table.set_status(Pid::new(pid), ProcessStatus::Running).await?;
        info!(pid, name, "agent spawned successfully");

        Ok(pid)
    }

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
                pid: entry.pid.as_u32(),
                name: entry.name,
                status,
                goal: entry.goal,
            });
        }
        Ok(active)
    }

    pub async fn abort_agent(&self, pid: u32) {
        info!(pid, "aborting agent");

        let mut handles = self.task_handles.lock().await;
        if let Some(handle) = handles.remove(&pid) {
            handle.abort();
            debug!(pid, "aborted executor task");
        } else {
            warn!(pid, "no executor task found for agent");
        }
        drop(handles);

        let _ = self.process_table.set_status(Pid::new(pid), ProcessStatus::Stopped).await;
        debug!(pid, "set process status to Stopped");

        self.finalize_invocation(pid, InvocationStatus::Killed, Some("killed".into())).await;
    }

    pub async fn list_installed(&self, username: &str) -> Vec<AgentManifestSummary> {
        match &self.manifest_scanner {
            Some(scanner) => scanner.scan(username).await,
            None => vec![],
        }
    }

    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    pub fn task_handles(&self) -> &Arc<Mutex<HashMap<u32, tokio::task::AbortHandle>>> {
        &self.task_handles
    }

    pub fn active_invocations(&self) -> &Arc<Mutex<HashMap<u32, String>>> {
        &self.active_invocations
    }

    pub fn active_sessions(&self) -> &Arc<Mutex<HashMap<u32, String>>> {
        &self.active_sessions
    }

    pub fn invocation_store(&self) -> Option<&Arc<InvocationStore>> {
        self.invocation_store.as_ref()
    }

    pub fn session_store(&self) -> Option<&Arc<PersistentSessionStore>> {
        self.session_store.as_ref()
    }

    async fn allocate_pid(&self) -> Result<u32, AvixError> {
        let entries = self.process_table.list_all().await;
        let max_pid = entries.iter().map(|e| e.pid.as_u32()).max().unwrap_or(1);
        Ok(max_pid + 1)
    }

    async fn finalize_invocation(
        &self,
        pid: u32,
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
        let (tokens, tool_calls) = match self.process_table.get(Pid::new(pid)).await {
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
        owner_pid: u32,
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
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&table),
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));

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
            signal_handler,
        );

        let pid1 = manager.spawn("agent-a", "goal-a", "sess-1", "kernel", None).await.unwrap();
        let pid2 = manager.spawn("agent-b", "goal-b", "sess-1", "kernel", None).await.unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(table.get(Pid::new(pid1)).await.unwrap().status, ProcessStatus::Running);
        assert_eq!(table.get(Pid::new(pid2)).await.unwrap().status, ProcessStatus::Running);

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
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&table),
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));

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
            signal_handler,
        );

        let pid = manager.spawn("agent", "goal", "sess", "kernel", None).await.unwrap();
        let entry = table.get(Pid::new(pid)).await.unwrap();
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
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&table),
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));

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
            signal_handler,
        );

        let pid1 = manager.spawn("agent1", "goal1", "sess-1", "kernel", None).await.unwrap();
        let pid2 = manager.spawn("agent2", "goal2", "sess-1", "kernel", None).await.unwrap();

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
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&table),
            None,
            Some(Arc::clone(&sstore)),
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));

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
            signal_handler,
        );

        let pid = manager.spawn("agent-a", "goal", "", "alice", None).await.unwrap();
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
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&table),
            None,
            Some(Arc::clone(&sstore)),
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));

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
            signal_handler,
        );

        let parent_pid = manager.spawn("parent-agent", "parent goal", "", "alice", None).await.unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        let parent_session_id = sessions[0].id;

        let child_pid = manager.spawn("child-agent", "child goal", "", "alice", Some(parent_pid)).await.unwrap();

        let session = sstore.get(&parent_session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&parent_pid));
        assert!(session.pids.contains(&child_pid));
    }
}
