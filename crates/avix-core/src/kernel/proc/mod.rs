use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::agent_manifest::{AgentManifestSummary, ManifestScanner};
use crate::error::AvixError;
use crate::executor::{AgentExecutorFactory, SpawnParams};
use crate::history::record::{MessageRecord, PartRecord};
use crate::history::HistoryStore;
use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::table::ProcessTable;
use crate::service::lifecycle::ServiceManager;
use crate::session::SessionRecord;
use crate::session::{PersistentSessionStore, SessionStatus};
use crate::tool_registry::ToolRegistry;
use crate::trace::Tracer;
use crate::types::token::{CapabilityToken, IssuedTo};
use crate::types::Pid;

pub mod agent;
pub mod history;
pub mod invocation;
pub mod persistence;
pub mod session;
pub mod signals;
pub mod types;

pub use agent::AgentManager;

use agent::AgentManager as InnerAgentManager;
pub use history::HistoryManager;
pub use invocation::InvocationManager;
pub use session::SessionManager;
pub use signals::SignalHandler;
pub use types::{
    ActiveAgent, AgentRecord, AgentsYaml, ServiceListResponse, ToolListResponse,
};

use persistence::{load_agents_yaml, persist_agent_record as persist_agent, save_agents_yaml};

/// Kernel proc domain handler.
/// Provides spawn, list, and persistence operations.
/// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
pub struct ProcHandler {
    process_table: Arc<ProcessTable>,
    agents_yaml_path: PathBuf,
    master_key: Vec<u8>,
    runtime_dir: PathBuf,
    executor_factory: Option<Arc<dyn AgentExecutorFactory>>,
    task_handles: Arc<Mutex<HashMap<u32, tokio::task::AbortHandle>>>,
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
    manifest_scanner: Option<Arc<ManifestScanner>>,
    active_invocations: Arc<Mutex<HashMap<u32, String>>>,
    active_sessions: Arc<Mutex<HashMap<u32, String>>>,
    service_manager: Arc<Mutex<Option<Arc<ServiceManager>>>>,
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,
    tracer: Arc<Tracer>,
    history_store: Option<Arc<HistoryStore>>,
    signal_handler: Arc<SignalHandler>,
    agent_manager: AgentManager,
}

impl ProcHandler {
    pub fn new(
        process_table: Arc<ProcessTable>,
        agents_yaml_path: PathBuf,
        master_key: Vec<u8>,
    ) -> Self {
        let runtime_dir = PathBuf::from("/run/avix");
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&process_table),
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));
        let agent_manager = InnerAgentManager::new(
            Arc::clone(&process_table),
            runtime_dir.clone(),
            master_key.clone(),
            None,
            None,
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::clone(&signal_handler),
        );
        Self {
            process_table,
            agents_yaml_path,
            master_key,
            runtime_dir,
            executor_factory: None,
            task_handles: Arc::new(Mutex::new(HashMap::new())),
            invocation_store: None,
            session_store: None,
            manifest_scanner: None,
            active_invocations,
            active_sessions,
            service_manager: Arc::new(Mutex::new(None)),
            tool_registry: Arc::new(Mutex::new(None)),
            tracer: Tracer::noop(),
            history_store: None,
            signal_handler,
            agent_manager,
        }
    }

    pub fn new_with_factory(
        process_table: Arc<ProcessTable>,
        agents_yaml_path: PathBuf,
        master_key: Vec<u8>,
        runtime_dir: PathBuf,
        factory: Arc<dyn AgentExecutorFactory>,
    ) -> Self {
        let active_invocations = Arc::new(Mutex::new(HashMap::new()));
        let active_sessions = Arc::new(Mutex::new(HashMap::new()));
        let signal_handler = Arc::new(SignalHandler::new(
            runtime_dir.clone(),
            Arc::clone(&process_table),
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
        ));
        let agent_manager = InnerAgentManager::new(
            Arc::clone(&process_table),
            runtime_dir.clone(),
            master_key.clone(),
            Some(factory),
            None,
            None,
            None,
            Arc::clone(&active_invocations),
            Arc::clone(&active_sessions),
            Arc::clone(&signal_handler),
        );
        Self {
            process_table,
            agents_yaml_path,
            master_key,
            runtime_dir,
            executor_factory: None,
            task_handles: Arc::new(Mutex::new(HashMap::new())),
            invocation_store: None,
            session_store: None,
            manifest_scanner: None,
            active_invocations,
            active_sessions,
            service_manager: Arc::new(Mutex::new(None)),
            tool_registry: Arc::new(Mutex::new(None)),
            tracer: Tracer::noop(),
            history_store: None,
            signal_handler,
            agent_manager,
        }
    }

    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = tracer;
        self
    }

    pub fn with_invocation_store(mut self, store: Arc<InvocationStore>) -> Self {
        self.invocation_store = Some(store.clone());
        
        self.signal_handler = Arc::new(SignalHandler::new(
            self.runtime_dir.clone(),
            Arc::clone(&self.process_table),
            Some(store.clone()),
            self.session_store.clone(),
            Arc::clone(&self.active_invocations),
            Arc::clone(&self.active_sessions),
        ));

        self.agent_manager = InnerAgentManager::new(
            Arc::clone(&self.process_table),
            self.runtime_dir.clone(),
            self.master_key.clone(),
            self.executor_factory.clone(),
            Some(store),
            self.session_store.clone(),
            self.manifest_scanner.clone(),
            Arc::clone(&self.active_invocations),
            Arc::clone(&self.active_sessions),
            Arc::clone(&self.signal_handler),
        );

        self
    }

    pub fn with_manifest_scanner(mut self, scanner: Arc<ManifestScanner>) -> Self {
        self.manifest_scanner = Some(scanner.clone());
        
        self.agent_manager = InnerAgentManager::new(
            Arc::clone(&self.process_table),
            self.runtime_dir.clone(),
            self.master_key.clone(),
            self.executor_factory.clone(),
            self.invocation_store.clone(),
            self.session_store.clone(),
            Some(scanner),
            Arc::clone(&self.active_invocations),
            Arc::clone(&self.active_sessions),
            Arc::clone(&self.signal_handler),
        );

        self
    }

    pub fn with_session_store(mut self, store: Arc<PersistentSessionStore>) -> Self {
        self.session_store = Some(store.clone());
        
        self.signal_handler = Arc::new(SignalHandler::new(
            self.runtime_dir.clone(),
            Arc::clone(&self.process_table),
            self.invocation_store.clone(),
            Some(store.clone()),
            Arc::clone(&self.active_invocations),
            Arc::clone(&self.active_sessions),
        ));

        self.agent_manager = InnerAgentManager::new(
            Arc::clone(&self.process_table),
            self.runtime_dir.clone(),
            self.master_key.clone(),
            self.executor_factory.clone(),
            self.invocation_store.clone(),
            Some(store),
            self.manifest_scanner.clone(),
            Arc::clone(&self.active_invocations),
            Arc::clone(&self.active_sessions),
            Arc::clone(&self.signal_handler),
        );

        self
    }

    pub fn with_history_store(mut self, store: Arc<HistoryStore>) -> Self {
        self.history_store = Some(store);
        self
    }

    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    pub async fn set_service_manager(&self, sm: Arc<ServiceManager>) {
        *self.service_manager.lock().await = Some(sm);
    }

    pub async fn set_tool_registry(&self, tr: Arc<ToolRegistry>) {
        *self.tool_registry.lock().await = Some(tr);
    }

    pub async fn list_services(&self) -> ServiceListResponse {
        if let Some(sm) = self.service_manager.lock().await.as_ref() {
            let services = sm.list_running().await;
            let running = services.iter().filter(|s| s.status == "running").count();
            let starting = services.iter().filter(|s| s.status == "starting").count();
            ServiceListResponse {
                total: services.len(),
                running,
                starting,
                services,
            }
        } else {
            warn!("service_manager not wired - returning empty service list");
            ServiceListResponse {
                total: 0,
                running: 0,
                starting: 0,
                services: vec![],
            }
        }
    }

    pub async fn list_tools(&self) -> ToolListResponse {
        if let Some(tr) = self.tool_registry.lock().await.as_ref() {
            let tools = tr.list_all().await;
            let available = tools.iter().filter(|t| t.state == "available").count();
            let unavailable = tools.iter().filter(|t| t.state != "available").count();
            ToolListResponse {
                total: tools.len(),
                available,
                unavailable,
                tools,
            }
        } else {
            warn!("tool_registry not wired - returning empty tool list");
            ToolListResponse {
                total: 0,
                available: 0,
                unavailable: 0,
                tools: vec![],
            }
        }
    }

    pub async fn abort_agent(&self, pid: u32) {
        self.agent_manager.abort_agent(pid).await;
        let mut handles = self.task_handles.lock().await;
        if let Some(handle) = handles.remove(&pid) {
            handle.abort();
        }
        let _ = self.process_table.set_status(Pid::new(pid), ProcessStatus::Stopped).await;
        self.finalize_invocation(pid, InvocationStatus::Killed, Some("killed".into())).await;
    }

    async fn finalize_invocation(
        &self,
        pid: u32,
        status: InvocationStatus,
        exit_reason: Option<String>,
    ) {
        let inv_id = {
            let mut map = self.active_invocations.lock().await;
            map.remove(&pid)
        };
        let inv_id = match inv_id {
            Some(id) => id,
            None => {
                self.finalize_session_for_pid(pid, &status).await;
                return;
            }
        };
        let store = match &self.invocation_store {
            Some(s) => s,
            None => {
                self.finalize_session_for_pid(pid, &status).await;
                return;
            }
        };
        let (tokens, tool_calls) = match self.process_table.get(Pid::new(pid)).await {
            Some(entry) => (entry.tokens_consumed, entry.tool_calls_total),
            None => (0, 0),
        };
        let _ = store
            .finalize(
                &inv_id,
                status.clone(),
                chrono::Utc::now(),
                tokens,
                tool_calls,
                exit_reason,
            )
            .await;
        self.finalize_session_for_pid(pid, &status).await;
    }

    pub async fn finalize_session_for_pid(&self, pid: u32, status: &InvocationStatus) {
        let session_id_str = self.active_sessions.lock().await.remove(&pid);
        let sid = match session_id_str {
            Some(s) => s,
            None => return,
        };
        let sstore = match &self.session_store {
            Some(s) => s,
            None => return,
        };
        let uuid = match Uuid::parse_str(&sid) {
            Ok(u) => u,
            Err(_) => return,
        };
        if let Ok(Some(mut session)) = sstore.get(&uuid).await {
            session.remove_pid(pid);
            if pid == session.owner_pid {
                match status {
                    InvocationStatus::Completed => session.mark_completed(),
                    InvocationStatus::Failed | InvocationStatus::Killed => session.mark_failed(),
                    _ => {}
                }
            }
            let _ = sstore.update(&session).await;
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
        let pid = self.agent_manager.spawn(name, goal, session_id, caller_identity, parent_pid).await?;

        persist_agent(&self.agents_yaml_path, pid, name, goal, session_id).await?;

        Ok(pid)
    }

    pub async fn list(&self) -> Result<Vec<ActiveAgent>, AvixError> {
        self.agent_manager.list().await
    }

    pub async fn list_installed(&self, username: &str) -> Vec<AgentManifestSummary> {
        self.agent_manager.list_installed(username).await
    }

    pub async fn list_invocations(
        &self,
        username: &str,
        agent_name: Option<&str>,
        live: bool,
    ) -> Result<Vec<InvocationRecord>, AvixError> {
        let store = match &self.invocation_store {
            Some(s) => s,
            None => return Ok(vec![]),
        };
        let records = match agent_name {
            Some(name) => store.list_for_agent(username, name).await?,
            None => store.list_for_user(username).await?,
        };
        if live {
            Ok(records)
        } else {
            Ok(records
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r.status,
                        InvocationStatus::Running
                            | InvocationStatus::Idle
                            | InvocationStatus::Paused
                    )
                })
                .collect())
        }
    }

    pub async fn get_invocation(
        &self,
        invocation_id: &str,
    ) -> Result<Option<InvocationRecord>, AvixError> {
        match &self.invocation_store {
            Some(s) => s.get(invocation_id).await,
            None => Ok(None),
        }
    }

    pub async fn snapshot_invocation(&self, id: &str) -> Result<InvocationRecord, AvixError> {
        let store = self
            .invocation_store
            .as_ref()
            .ok_or_else(|| AvixError::NotFound("invocation store not configured".into()))?;

        let record = store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {id} not found")))?;

        if !matches!(
            record.status,
            InvocationStatus::Running | InvocationStatus::Idle | InvocationStatus::Paused
        ) {
            return Err(AvixError::InvalidInput(
                "cannot snapshot a finalized invocation".into(),
            ));
        }

        store
            .persist_interim(id, &[], record.tokens_consumed, record.tool_calls_total)
            .await?;

        store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {id} not found")))
    }

    pub async fn create_message(&self, msg: &MessageRecord) -> Result<(), AvixError> {
        match &self.history_store {
            Some(s) => s.create_message(msg).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    pub async fn get_message(&self, id: &Uuid) -> Result<Option<MessageRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.get_message(id).await,
            None => Ok(None),
        }
    }

    pub async fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.list_messages(session_id).await,
            None => Ok(vec![]),
        }
    }

    pub async fn create_part(&self, part: &PartRecord) -> Result<(), AvixError> {
        match &self.history_store {
            Some(s) => s.create_part(part).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    pub async fn get_part(&self, id: &Uuid) -> Result<Option<PartRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.get_part(id).await,
            None => Ok(None),
        }
    }

    pub async fn list_parts(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.list_parts(message_id).await,
            None => Ok(vec![]),
        }
    }

    pub async fn create_session(
        &self,
        username: &str,
        origin_agent: &str,
        title: &str,
        goal: &str,
        owner_pid: u32,
    ) -> Result<SessionRecord, AvixError> {
        let store = match &self.session_store {
            Some(s) => s,
            None => return Err(AvixError::NotFound("session store not configured".into())),
        };
        let record = SessionRecord::new(
            Uuid::new_v4(),
            username.to_string(),
            origin_agent.to_string(),
            title.to_string(),
            goal.to_string(),
            owner_pid,
        );
        store.create(&record).await?;
        info!(session_id = %record.id, "created session");
        Ok(record)
    }

    pub async fn list_sessions(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        match &self.session_store {
            Some(s) => s.list_for_user(username).await,
            None => Ok(vec![]),
        }
    }

    pub async fn get_session(&self, session_id: &Uuid) -> Result<Option<SessionRecord>, AvixError> {
        match &self.session_store {
            Some(s) => s.get(session_id).await,
            None => Ok(None),
        }
    }

    pub async fn resume_session(
        &self,
        session_id: &Uuid,
        input: Option<&str>,
    ) -> Result<u32, AvixError> {
        let store = match &self.session_store {
            Some(s) => s,
            None => return Err(AvixError::NotFound("session store not configured".into())),
        };

        let session = store
            .get(session_id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("session {} not found", session_id)))?;

        if !matches!(
            session.status,
            SessionStatus::Idle | SessionStatus::Running | SessionStatus::Paused
        ) {
            return Err(AvixError::InvalidInput(format!(
                "session {} is not Idle, Running, or Paused (status: {:?})",
                session_id, session.status
            )));
        }

        let goal = input.unwrap_or(&session.goal).to_string();

        let pid = self
            .spawn(
                &session.primary_agent,
                &goal,
                &session_id.to_string(),
                &session.username,
                None,
            )
            .await?;

        info!(session_id = %session_id, pid, "resumed session");
        Ok(pid)
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
                let record = SessionRecord::new(
                    Uuid::new_v4(),
                    caller_identity.to_string(),
                    name.to_string(),
                    name.to_string(),
                    goal.to_string(),
                    owner_pid,
                );
                if let Err(e) = store.create(&record).await {
                    warn!(error = %e, "failed to create session record");
                }
                info!(session_id = %record.id, owner_pid, "created new session");
                Ok(record.id.to_string())
            } else {
                Ok(Uuid::new_v4().to_string())
            }
        } else {
            if let Some(store) = &self.session_store {
                if let Ok(Some(mut session)) = store.get(&Uuid::parse_str(session_id)?).await {
                    session.add_participant(name, true);
                    if let Err(e) = store.update(&session).await {
                        warn!(error = %e, "failed to update session with participant");
                    }
                    info!(session_id = %session.id, participant = name, "added participant to session");
                }
            }
            Ok(session_id.to_string())
        }
    }

    pub async fn pause_agent(&self, pid: u32) -> Result<(), AvixError> {
        self.signal_handler.pause_agent(pid).await
    }

    pub async fn resume_agent(&self, pid: u32) -> Result<(), AvixError> {
        self.signal_handler.resume_agent(pid).await
    }

    pub async fn send_signal(
        &self,
        pid: u32,
        signal: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
        self.signal_handler.send_signal(pid, signal, payload).await
    }

    async fn allocate_pid(&self) -> Result<u32, AvixError> {
        let entries = self.process_table.list_all().await;
        let max_pid = entries.iter().map(|e| e.pid.as_u32()).max().unwrap_or(1);
        Ok(max_pid + 1)
    }

    pub async fn load_agents_yaml(&self) -> Result<AgentsYaml, AvixError> {
        load_agents_yaml(&self.agents_yaml_path).await
    }

    pub async fn remove_agent_record(&self, pid: u32) -> Result<(), AvixError> {
        let mut agents = load_agents_yaml(&self.agents_yaml_path).await.unwrap_or_default();
        agents.agents.retain(|a| a.pid != pid);
        save_agents_yaml(&self.agents_yaml_path, &agents).await?;
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
            .spawn("agent-a", "goal-a", "sess-1", "kernel", None)
            .await
            .unwrap();
        let pid2 = handler
            .spawn("agent-b", "goal-b", "sess-1", "kernel", None)
            .await
            .unwrap();

        assert_eq!(count.load(Ordering::SeqCst), 2);

        assert_eq!(
            table.get(Pid::new(pid1)).await.unwrap().status,
            ProcessStatus::Running
        );
        assert_eq!(
            table.get(Pid::new(pid2)).await.unwrap().status,
            ProcessStatus::Running
        );

        handler.abort_agent(pid1).await;
        
        let p1_status = table.get(Pid::new(pid1)).await.unwrap().status;
        let p2_status = table.get(Pid::new(pid2)).await.unwrap().status;
        assert_eq!(p1_status, ProcessStatus::Stopped);
        assert_eq!(p2_status, ProcessStatus::Running);
    }

    #[tokio::test]
    async fn spawn_without_factory_still_registers_process() {
        let dir = TempDir::new().unwrap();
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table.clone(), dir.path().join("agents.yaml"), master_key);

        let pid = handler
            .spawn("agent", "goal", "sess", "kernel", None)
            .await
            .unwrap();
        let entry = table.get(Pid::new(pid)).await.unwrap();
        assert_eq!(entry.status, ProcessStatus::Running);
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
            .spawn("test_agent", "test_goal", "sess-1", "kernel", None)
            .await
            .unwrap();

        let entry = table.get(Pid::new(pid)).await.unwrap();
        assert_eq!(entry.name, "test_agent");
        assert_eq!(entry.goal, "test_goal");
        assert_eq!(entry.status, ProcessStatus::Running);

        let yaml: AgentsYaml =
            serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
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

        let pid1 = handler
            .spawn("agent1", "goal1", "sess-1", "kernel", None)
            .await
            .unwrap();
        let pid2 = handler
            .spawn("agent2", "goal2", "sess-1", "kernel", None)
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
            .spawn("test", "goal", "sess", "kernel", None)
            .await
            .unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 1);

        handler.remove_agent_record(pid).await.unwrap();
        assert_eq!(handler.load_agents_yaml().await.unwrap().agents.len(), 0);
    }

    #[tokio::test]
    async fn list_services_returns_empty_response_when_not_wired() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table, yaml_path, master_key);

        let response = handler.list_services().await;
        assert_eq!(response.total, 0);
        assert_eq!(response.running, 0);
        assert_eq!(response.starting, 0);
        assert!(response.services.is_empty());
    }

    #[tokio::test]
    async fn list_tools_returns_empty_response_when_not_wired() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let handler = ProcHandler::new(table, yaml_path, master_key);

        let response = handler.list_tools().await;
        assert_eq!(response.total, 0);
        assert_eq!(response.available, 0);
        assert_eq!(response.unavailable, 0);
        assert!(response.tools.is_empty());
    }

    #[tokio::test]
    async fn service_list_response_serializes_to_json() {
        let response = ServiceListResponse {
            total: 5,
            running: 3,
            starting: 2,
            services: vec![crate::service::ServiceSummary {
                name: "test-svc".to_string(),
                pid: 42,
                status: "running".to_string(),
                registered_at: None,
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"total\":5"));
        assert!(json.contains("\"running\":3"));
        assert!(json.contains("\"starting\":2"));
        assert!(json.contains("\"test-svc\""));
    }

    #[tokio::test]
    async fn tool_list_response_serializes_to_json() {
        let response = ToolListResponse {
            total: 10,
            available: 8,
            unavailable: 2,
            tools: vec![crate::tool_registry::ToolSummary {
                name: "fs/read".to_string(),
                namespace: "fs".to_string(),
                description: "Read a file".to_string(),
                state: "available".to_string(),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"total\":10"));
        assert!(json.contains("\"available\":8"));
        assert!(json.contains("\"unavailable\":2"));
        assert!(json.contains("\"fs/read\""));
    }

    #[tokio::test]
    async fn service_list_response_deserializes_from_json() {
        let json = r#"{"total":3,"running":2,"starting":1,"services":[{"name":"svc1","pid":10,"status":"running","registered_at":null}]}"#;
        let response: ServiceListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.total, 3);
        assert_eq!(response.running, 2);
        assert_eq!(response.starting, 1);
        assert_eq!(response.services.len(), 1);
        assert_eq!(response.services[0].name, "svc1");
    }

    #[tokio::test]
    async fn tool_list_response_deserializes_from_json() {
        let json = r#"{"total":5,"available":4,"unavailable":1,"tools":[{"name":"test/tool","namespace":"test","description":"desc","state":"available"}]}"#;
        let response: ToolListResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.total, 5);
        assert_eq!(response.available, 4);
        assert_eq!(response.unavailable, 1);
        assert_eq!(response.tools.len(), 1);
        assert_eq!(response.tools[0].name, "test/tool");
    }

    async fn make_handler_with_stores(
        dir: &TempDir,
    ) -> (
        ProcHandler,
        Arc<PersistentSessionStore>,
        Arc<InvocationStore>,
    ) {
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        let sstore = Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        );
        let istore = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let handler = ProcHandler::new(table, yaml_path, master_key)
            .with_session_store(Arc::clone(&sstore))
            .with_invocation_store(Arc::clone(&istore));
        (handler, sstore, istore)
    }

    #[tokio::test]
    async fn spawn_without_parent_pid_creates_new_session() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent-a", "goal", "", "alice", None)
            .await
            .unwrap();

        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].pids.contains(&pid));
        assert_eq!(sessions[0].owner_pid, pid);
    }

    #[tokio::test]
    async fn spawn_with_parent_pid_inherits_session() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        let parent_pid = handler
            .spawn("parent-agent", "parent goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        let parent_session_id = sessions[0].id;

        let child_pid = handler
            .spawn("child-agent", "child goal", "", "alice", Some(parent_pid))
            .await
            .unwrap();

        let session = sstore.get(&parent_session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&parent_pid));
        assert!(session.pids.contains(&child_pid));
        assert_eq!(sstore.list_for_user("alice").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn finalize_invocation_removes_pid_from_session() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent", "goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        let session_id = sessions[0].id;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&pid));

        handler.abort_agent(pid).await;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert!(!session.pids.contains(&pid));
    }

    #[tokio::test]
    async fn finalize_invocation_marks_session_completed_on_owner_exit() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, istore) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent", "goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        let session_id = sessions[0].id;

        let inv_id = handler
            .active_invocations
            .lock()
            .await
            .get(&pid)
            .cloned()
            .unwrap();
        let _ = istore
            .finalize(
                &inv_id,
                InvocationStatus::Completed,
                chrono::Utc::now(),
                0,
                0,
                None,
            )
            .await;
        handler
            .finalize_session_for_pid(pid, &InvocationStatus::Completed)
            .await;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, crate::session::SessionStatus::Completed);
    }

    #[tokio::test]
    async fn finalize_invocation_marks_session_failed_on_owner_kill() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent", "goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        let session_id = sessions[0].id;

        handler.abort_agent(pid).await;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, crate::session::SessionStatus::Failed);
    }

    #[tokio::test]
    async fn finalize_invocation_does_not_transition_session_on_non_owner_exit() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        let owner_pid = handler
            .spawn("owner", "goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        let session_id = sessions[0].id;

        let child_pid = handler
            .spawn("child", "subgoal", "", "alice", Some(owner_pid))
            .await
            .unwrap();

        handler
            .finalize_session_for_pid(child_pid, &InvocationStatus::Completed)
            .await;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, crate::session::SessionStatus::Running);
    }

    #[tokio::test]
    async fn list_invocations_excludes_paused_when_live_false() {
        let dir = TempDir::new().unwrap();
        let (handler, _, istore) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent", "goal", "", "alice", None)
            .await
            .unwrap();
        let inv_id = handler
            .active_invocations
            .lock()
            .await
            .get(&pid)
            .cloned()
            .unwrap();

        istore
            .update_status(&inv_id, InvocationStatus::Paused)
            .await
            .unwrap();

        let records = handler
            .list_invocations("alice", None, false)
            .await
            .unwrap();
        assert!(records.is_empty());

        let records = handler.list_invocations("alice", None, true).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, InvocationStatus::Paused);
    }

    #[tokio::test]
    async fn snapshot_invocation_allows_paused() {
        let dir = TempDir::new().unwrap();
        let (handler, _, istore) = make_handler_with_stores(&dir).await;

        let pid = handler
            .spawn("agent", "goal", "", "alice", None)
            .await
            .unwrap();
        let inv_id = handler
            .active_invocations
            .lock()
            .await
            .get(&pid)
            .cloned()
            .unwrap();

        istore
            .update_status(&inv_id, InvocationStatus::Paused)
            .await
            .unwrap();

        let result = handler.snapshot_invocation(&inv_id).await;
        assert!(result.is_ok());
    }
}