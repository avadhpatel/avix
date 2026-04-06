use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
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
use crate::service::ServiceSummary;
use crate::session::SessionRecord;
use crate::session::{PersistentSessionStore, SessionStatus};
use crate::signal::{Signal, SignalDelivery, SignalKind};
use crate::tool_registry::{ToolRegistry, ToolSummary};
use crate::trace::Tracer;
use crate::types::token::{CapabilityToken, IssuedTo};
use crate::types::Pid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListResponse {
    pub total: usize,
    pub available: usize,
    pub unavailable: usize,
    pub tools: Vec<ToolSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceListResponse {
    pub total: usize,
    pub running: usize,
    pub starting: usize,
    pub services: Vec<ServiceSummary>,
}

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
    /// Persistent store for agent invocation records (optional).
    invocation_store: Option<Arc<InvocationStore>>,
    /// Persistent store for session records (optional).
    session_store: Option<Arc<PersistentSessionStore>>,
    /// Scanner for discovering installed agent manifests (optional).
    manifest_scanner: Option<Arc<ManifestScanner>>,
    /// Maps running PID → invocation UUID, for finalization on kill.
    active_invocations: Arc<Mutex<HashMap<u32, String>>>,
    /// Maps running PID → session UUID string, for session lookup on pause/finalize.
    active_sessions: Arc<Mutex<HashMap<u32, String>>>,
    /// Service manager — set in phase3 after services start.
    service_manager: Arc<Mutex<Option<Arc<ServiceManager>>>>,
    /// Tool registry — set in phase3 after services start.
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,
    /// Tracer — when set, agent spawn events are written to the agent trace file.
    tracer: Arc<Tracer>,
    /// History store for MessageRecord / PartRecord (optional).
    history_store: Option<Arc<HistoryStore>>,
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
            invocation_store: None,
            session_store: None,
            manifest_scanner: None,
            active_invocations: Arc::new(Mutex::new(HashMap::new())),
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            service_manager: Arc::new(Mutex::new(None)),
            tool_registry: Arc::new(Mutex::new(None)),
            tracer: Tracer::noop(),
            history_store: None,
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
            invocation_store: None,
            session_store: None,
            manifest_scanner: None,
            active_invocations: Arc::new(Mutex::new(HashMap::new())),
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            service_manager: Arc::new(Mutex::new(None)),
            tool_registry: Arc::new(Mutex::new(None)),
            tracer: Tracer::noop(),
            history_store: None,
        }
    }

    /// Attach a tracer to record agent spawn events.
    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = tracer;
        self
    }

    /// Attach a persistent invocation store.
    pub fn with_invocation_store(mut self, store: Arc<InvocationStore>) -> Self {
        self.invocation_store = Some(store);
        self
    }

    /// Attach a manifest scanner for agent discovery.
    pub fn with_manifest_scanner(mut self, scanner: Arc<ManifestScanner>) -> Self {
        self.manifest_scanner = Some(scanner);
        self
    }

    /// Attach a persistent session store.
    pub fn with_session_store(mut self, store: Arc<PersistentSessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Attach a history store for MessageRecord / PartRecord.
    pub fn with_history_store(mut self, store: Arc<HistoryStore>) -> Self {
        self.history_store = Some(store);
        self
    }

    /// Expose the process table for use by other kernel subsystems (e.g. ipc_server).
    pub fn process_table(&self) -> &Arc<ProcessTable> {
        &self.process_table
    }

    /// Wire in the `ServiceManager` after phase3 services start.
    pub async fn set_service_manager(&self, sm: Arc<ServiceManager>) {
        *self.service_manager.lock().await = Some(sm);
    }

    /// Wire in the `ToolRegistry` after phase3 services start.
    pub async fn set_tool_registry(&self, tr: Arc<ToolRegistry>) {
        *self.tool_registry.lock().await = Some(tr);
    }

    /// List all running services. Returns response with metadata.
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

    /// List all registered tools. Returns response with metadata.
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
        drop(handles);
        self.finalize_invocation(pid, InvocationStatus::Killed, Some("killed".into()))
            .await;
    }

    /// Finalize the invocation record for a PID (called on kill or normal exit).
    /// Also removes the PID from the session and transitions session status if the
    /// owner PID is the one exiting.
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
                // Still clean up session tracking even without an invocation record.
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
        // Read final metrics from the process table (best-effort).
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

    /// Remove `pid` from its session's active PID set. If the pid is the session
    /// owner, transition the session to a terminal state based on the invocation status.
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
                    _ => {} // Idle/Paused/Running do not finalize the session
                }
            }
            let _ = sstore.update(&session).await;
        }
    }

    /// Spawn a new agent: allocate PID, mint CapToken, write /proc/ files, persist to agents.yaml, fork/exec RuntimeExecutor.
    /// Returns the allocated PID.
    ///
    /// If `parent_pid` is `Some`, the new agent inherits the parent's session. Otherwise
    /// a new session is created (or the provided `session_id` is used).
    /// Links: docs/architecture/06-agents.md, docs/architecture/14-agent-persistence.md
    pub async fn spawn(
        &self,
        name: &str,
        goal: &str,
        session_id: &str,
        caller_identity: &str,
        parent_pid: Option<u32>,
    ) -> Result<u32, AvixError> {
        info!(name, goal, session_id, ?parent_pid, "spawning agent");

        // Allocate PID first so it can be recorded as session owner_pid.
        let pid = self.allocate_pid().await?;
        info!(pid, "allocated PID");

        // Resolve session: inherit from parent, attach to existing, or create new.
        let effective_session_id = if let Some(ppid) = parent_pid {
            // Try to inherit the parent's session.
            let inherited = self.active_sessions.lock().await.get(&ppid).cloned();
            if let Some(sid) = inherited {
                // Attach new agent as participant in the inherited session.
                if let Some(store) = &self.session_store {
                    if let Ok(Some(mut session)) = store.get(&Uuid::parse_str(&sid)?).await {
                        session.add_participant(name, true);
                        if let Err(e) = store.update(&session).await {
                            warn!(error = %e, "failed to update session with child participant");
                        }
                    }
                }
                info!(session_id = %sid, parent_pid = ppid, "child agent inheriting parent session");
                sid
            } else {
                warn!(
                    parent_pid = ppid,
                    "parent pid not found in active_sessions; creating new session"
                );
                self.resolve_session_from_id(name, goal, session_id, caller_identity, pid)
                    .await?
            }
        } else {
            self.resolve_session_from_id(name, goal, session_id, caller_identity, pid)
                .await?
        };

        // Create process entry
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

        // Insert into process table
        self.process_table.insert(entry).await;

        // Persist to agents.yaml
        self.persist_agent_record(pid, name, goal, &effective_session_id)
            .await?;
        info!(pid, "persisted agent record to agents.yaml");

        // Write /proc/<pid>/status.yaml and resolved.yaml
        // TODO: Implement init_proc_files here

        // Register PID in the session's active PID list.
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
        self.active_sessions
            .lock()
            .await
            .insert(pid, effective_session_id.clone());

        // Create invocation record (before minting token / launching executor)
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
                warn!(pid, invocation_id = %invocation_id, error = %e, "failed to create invocation record");
            }
        }
        self.active_invocations
            .lock()
            .await
            .insert(pid, invocation_id.clone());

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
                session_id: effective_session_id.clone(),
                token,
                system_prompt: None,
                selected_model: String::new(), // factory resolves via llm.svc
                denied_tools: vec![],
                context_limit: 0,
                runtime_dir: self.runtime_dir.clone(),
                invocation_id: invocation_id.clone(),
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

    // ── Discovery / history API ───────────────────────────────────────────────

    /// List all agents installed and available to `username`.
    /// Returns empty vec if no manifest scanner is configured.
    pub async fn list_installed(&self, username: &str) -> Vec<AgentManifestSummary> {
        match &self.manifest_scanner {
            Some(scanner) => scanner.scan(username).await,
            None => vec![],
        }
    }

    /// List historical invocations for `username`, optionally filtered by agent name.
    ///
    /// When `live=true`, all records (including Running/Idle) are returned.
    /// When `live=false`, only finalized records (Completed/Failed/Killed) are returned.
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

    /// Get a specific invocation record by UUID.
    ///
    /// The `live` parameter is reserved for v2.1 (runtime state merge).
    /// In v2.0, it is ignored and the persisted record is always returned.
    pub async fn get_invocation(
        &self,
        invocation_id: &str,
    ) -> Result<Option<InvocationRecord>, AvixError> {
        match &self.invocation_store {
            Some(s) => s.get(invocation_id).await,
            None => Ok(None),
        }
    }

    /// Force an immediate snapshot of a running invocation.
    ///
    /// Calls `persist_interim` with current stats from the invocation record.
    /// Returns the updated record.
    ///
    /// Returns `AvixError::NotFound` if `id` is unknown.
    /// Returns `AvixError::InvalidInput` if the invocation is already finalized.
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

    // ── History (MessageRecord / PartRecord) ──────────────────────────────────

    /// Create a message in the history store.
    pub async fn create_message(&self, msg: &MessageRecord) -> Result<(), AvixError> {
        match &self.history_store {
            Some(s) => s.create_message(msg).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    /// Get a message by UUID.
    pub async fn get_message(&self, id: &Uuid) -> Result<Option<MessageRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.get_message(id).await,
            None => Ok(None),
        }
    }

    /// List messages for a session, ordered by sequence.
    pub async fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.list_messages(session_id).await,
            None => Ok(vec![]),
        }
    }

    /// Create a part in the history store.
    pub async fn create_part(&self, part: &PartRecord) -> Result<(), AvixError> {
        match &self.history_store {
            Some(s) => s.create_part(part).await,
            None => Err(AvixError::NotFound("history store not configured".into())),
        }
    }

    /// Get a part by UUID.
    pub async fn get_part(&self, id: &Uuid) -> Result<Option<PartRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.get_part(id).await,
            None => Ok(None),
        }
    }

    /// List parts for a message, ordered by part_index.
    pub async fn list_parts(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        match &self.history_store {
            Some(s) => s.list_parts(message_id).await,
            None => Ok(vec![]),
        }
    }

    // ── Session operations ─────────────────────────────────────────────────────

    /// Create a new session.
    ///
    /// `owner_pid` must be a valid PID. Pass the PID that will own this session;
    /// if the session is pre-created before any spawn, use the PID that will be spawned into it.
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

    /// List all sessions for a user.
    pub async fn list_sessions(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        match &self.session_store {
            Some(s) => s.list_for_user(username).await,
            None => Ok(vec![]),
        }
    }

    /// Get a specific session by ID.
    pub async fn get_session(&self, session_id: &Uuid) -> Result<Option<SessionRecord>, AvixError> {
        match &self.session_store {
            Some(s) => s.get(session_id).await,
            None => Ok(None),
        }
    }

    /// Resume a session by spawning a new invocation in it.
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

        // Only allow resuming Idle, Running, or Paused sessions
        if !matches!(
            session.status,
            SessionStatus::Idle | SessionStatus::Running | SessionStatus::Paused
        ) {
            return Err(AvixError::InvalidInput(format!(
                "session {} is not Idle, Running, or Paused (status: {:?})",
                session_id, session.status
            )));
        }

        // Build the goal from input or use session's goal
        let goal = input.unwrap_or(&session.goal).to_string();

        // Spawn the agent in this session - this will:
        // 1. Attach to existing session
        // 2. Update primary_agent to the new agent
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

    /// Resolve a session for spawn when there is no `parent_pid`.
    /// - If `session_id` is non-empty → attach to that session.
    /// - Otherwise → create a new session with `owner_pid` set to the spawning PID.
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
            // Attach to existing session.
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

    /// Pause a running agent: update process table, mark invocation `Paused`,
    /// deliver SIGPAUSE, and — if this is the session owner — cascade to all
    /// other PIDs in the session and mark the session `Paused`.
    pub async fn pause_agent(&self, pid: u32) -> Result<(), AvixError> {
        // 1. Process table → Paused.
        let _ = self
            .process_table
            .set_status(Pid::new(pid), ProcessStatus::Paused)
            .await;

        // 2. Invocation → Paused (look up inv_id first, drop lock before session work).
        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            let _ = istore.update_status(&id, InvocationStatus::Paused).await;
        }

        // 3. Deliver SIGPAUSE to the agent socket.
        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let signal = Signal {
            target: Pid::new(pid),
            kind: SignalKind::Pause,
            payload: serde_json::Value::Null,
        };
        // Best-effort: socket may not exist if agent hasn't started yet.
        let _ = delivery.deliver(signal).await;

        // 4. Check if this is the session owner → cascade to all other PIDs.
        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if pid == session.owner_pid {
                        // Collect other PIDs before any async work (avoid holding lock).
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::new(p))
                            .collect();
                        // Broadcast SIGPAUSE to all sibling PIDs concurrently.
                        if !other_pids.is_empty() {
                            delivery
                                .broadcast(&other_pids, SignalKind::Pause, serde_json::Value::Null)
                                .await;
                            // Update process table and invocations for each cascaded PID.
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Paused)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u32())
                                    .cloned();
                                if let (Some(iid), Some(istore)) =
                                    (sibling_inv, &self.invocation_store)
                                {
                                    let _ =
                                        istore.update_status(&iid, InvocationStatus::Paused).await;
                                }
                            }
                        }
                        session.mark_paused();
                        let _ = sstore.update(&session).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// Resume a paused agent: update process table, mark invocation `Running`,
    /// deliver SIGRESUME, and — if the session is `Paused` — cascade to all
    /// other PIDs and mark the session `Running`.
    pub async fn resume_agent(&self, pid: u32) -> Result<(), AvixError> {
        // 1. Process table → Running.
        let _ = self
            .process_table
            .set_status(Pid::new(pid), ProcessStatus::Running)
            .await;

        // 2. Invocation → Running.
        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            let _ = istore.update_status(&id, InvocationStatus::Running).await;
        }

        // 3. Deliver SIGRESUME.
        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let signal = Signal {
            target: Pid::new(pid),
            kind: SignalKind::Resume,
            payload: serde_json::Value::Null,
        };
        let _ = delivery.deliver(signal).await;

        // 4. If session is Paused, cascade SIGRESUME to all other PIDs and mark Running.
        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if matches!(session.status, SessionStatus::Paused) {
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::new(p))
                            .collect();
                        if !other_pids.is_empty() {
                            delivery
                                .broadcast(&other_pids, SignalKind::Resume, serde_json::Value::Null)
                                .await;
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Running)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u32())
                                    .cloned();
                                if let (Some(iid), Some(istore)) =
                                    (sibling_inv, &self.invocation_store)
                                {
                                    let _ =
                                        istore.update_status(&iid, InvocationStatus::Running).await;
                                }
                            }
                        }
                        session.mark_running();
                        let _ = sstore.update(&session).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// Deliver an arbitrary signal to a PID.
    ///
    /// SIGPAUSE and SIGRESUME are routed through `pause_agent` / `resume_agent` so that
    /// process-table, invocation-store, and session-cascade side-effects are applied.
    /// All other signals are delivered directly via `SignalDelivery`.
    pub async fn send_signal(
        &self,
        pid: u32,
        signal: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
        match signal {
            "SIGPAUSE" => return self.pause_agent(pid).await,
            "SIGRESUME" => return self.resume_agent(pid).await,
            _ => {}
        }
        let kind = match signal {
            "SIGSTART" => SignalKind::Start,
            "SIGKILL" => SignalKind::Kill,
            "SIGSTOP" => SignalKind::Stop,
            "SIGSAVE" => SignalKind::Save,
            "SIGPIPE" => SignalKind::Pipe,
            "SIGESCALATE" => SignalKind::Escalate,
            other => {
                return Err(AvixError::ConfigParse(format!(
                    "unknown signal: {other}"
                )))
            }
        };
        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let sig = Signal {
            target: Pid::new(pid),
            kind,
            payload,
        };
        let _ = delivery.deliver(sig).await;
        Ok(())
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
            .spawn("agent-a", "goal-a", "sess-1", "kernel", None)
            .await
            .unwrap();
        let pid2 = handler
            .spawn("agent-b", "goal-b", "sess-1", "kernel", None)
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
            .spawn("agent", "goal", "sess", "kernel", None)
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
            .spawn("test_agent", "test_goal", "sess-1", "kernel", None)
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
            services: vec![ServiceSummary {
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
            tools: vec![ToolSummary {
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

    // ── Session / parent_pid tests ────────────────────────────────────────────

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

        // Session should be created and PID registered in it.
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].pids.contains(&pid));
        assert_eq!(sessions[0].owner_pid, pid);
    }

    #[tokio::test]
    async fn spawn_with_parent_pid_inherits_session() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        // Spawn parent — creates new session.
        let parent_pid = handler
            .spawn("parent-agent", "parent goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        let parent_session_id = sessions[0].id;

        // Spawn child with parent_pid — should inherit parent session.
        let child_pid = handler
            .spawn("child-agent", "child goal", "", "alice", Some(parent_pid))
            .await
            .unwrap();

        let session = sstore.get(&parent_session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&parent_pid));
        assert!(session.pids.contains(&child_pid));
        // Only one session should exist.
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

        // Verify pid is in session before finalization.
        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert!(session.pids.contains(&pid));

        // Finalize (simulate kill).
        handler.abort_agent(pid).await;

        // PID should be removed from session.
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

        // Finalize as Completed (simulate normal exit).
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

        // abort_agent calls finalize_invocation with Killed.
        handler.abort_agent(pid).await;

        let session = sstore.get(&session_id).await.unwrap().unwrap();
        assert_eq!(session.status, crate::session::SessionStatus::Failed);
    }

    #[tokio::test]
    async fn finalize_invocation_does_not_transition_session_on_non_owner_exit() {
        let dir = TempDir::new().unwrap();
        let (handler, sstore, _) = make_handler_with_stores(&dir).await;

        // Spawn owner.
        let owner_pid = handler
            .spawn("owner", "goal", "", "alice", None)
            .await
            .unwrap();
        let sessions = sstore.list_for_user("alice").await.unwrap();
        let session_id = sessions[0].id;

        // Spawn child in same session.
        let child_pid = handler
            .spawn("child", "subgoal", "", "alice", Some(owner_pid))
            .await
            .unwrap();

        // Child completes — session should still be Running (owner hasn't exited).
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

        // Mark invocation as Paused.
        istore
            .update_status(&inv_id, InvocationStatus::Paused)
            .await
            .unwrap();

        // live=false should exclude Paused.
        let records = handler
            .list_invocations("alice", None, false)
            .await
            .unwrap();
        assert!(records.is_empty());

        // live=true should include Paused.
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

        // Mark as Paused.
        istore
            .update_status(&inv_id, InvocationStatus::Paused)
            .await
            .unwrap();

        // snapshot_invocation should succeed for Paused.
        let result = handler.snapshot_invocation(&inv_id).await;
        assert!(result.is_ok());
    }
}
