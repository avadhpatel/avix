// Child modules — declared here so they share this module's privacy scope
// and can access private RuntimeExecutor fields.
mod dispatch_manager;
mod proc_manager;

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

use crate::error::AvixError;
use crate::gateway::event_bus::AtpEventBus;
use crate::kernel::resource_request::KernelResourceHandler;
use crate::llm_client::LlmCompleteResponse;
use crate::memfs::context::VfsCallerContext;
use crate::memfs::VfsRouter;
use crate::memory_svc::vfs_layout::init_user_memory_tree;
use crate::signal::kind::Signal;
use crate::tool_registry::ToolRegistry;
use crate::trace::Tracer;
use crate::types::{token::CapabilityToken, tool::ToolVisibility, Pid};

use super::memory::MemoryManager;
use super::mock_kernel::MockKernelHandle;
use super::prompt::build_system_prompt;
use super::spawn::SpawnParams;
use super::tool_manager::ToolManager;
use super::tool_registration::compute_cat2_tools;

/// Minimal trait that MockToolRegistry satisfies
pub trait ToolRegistryHandle: Send + Sync {
    fn register_tool(
        &self,
        pid: u64,
        name: &str,
        visibility: ToolVisibility,
    ) -> impl std::future::Future<Output = ()> + Send;

    fn deregister_tool(&self, pid: u64, name: &str)
        -> impl std::future::Future<Output = ()> + Send;

    /// Look up a tool descriptor by name. Returns `None` if not found.
    fn lookup_descriptor(
        &self,
        name: &str,
    ) -> impl std::future::Future<Output = Option<serde_json::Value>> + Send;
}

/// Concrete: the mock registry used in tests
pub struct MockToolRegistry {
    pub registered: Arc<Mutex<Vec<(u64, String, ToolVisibility)>>>,
}

impl MockToolRegistry {
    pub fn new() -> Self {
        Self {
            registered: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn tools_registered_by_pid(&self, pid: u64) -> HashSet<String> {
        self.registered
            .lock()
            .await
            .iter()
            .filter(|(p, _, _)| *p == pid)
            .map(|(_, name, _)| name.clone())
            .collect()
    }

    pub async fn all_registered(&self) -> Vec<(u64, String)> {
        self.registered
            .lock()
            .await
            .iter()
            .map(|(p, n, _)| (*p, n.clone()))
            .collect()
    }
}

impl Default for MockToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistryHandle for Arc<ToolRegistry> {
    async fn register_tool(&self, _pid: u64, _name: &str, _visibility: ToolVisibility) {
        // No-op: real ToolRegistry manages registration via ipc.tool-add at kernel level.
    }

    async fn deregister_tool(&self, _pid: u64, _name: &str) {
        // No-op: real ToolRegistry manages deregistration via ipc.tool-remove at kernel level.
    }

    async fn lookup_descriptor(&self, name: &str) -> Option<serde_json::Value> {
        self.lookup(name).await.ok().map(|e| e.descriptor)
    }
}

impl ToolRegistryHandle for Arc<MockToolRegistry> {
    async fn register_tool(&self, pid: u64, name: &str, visibility: ToolVisibility) {
        self.registered
            .lock()
            .await
            .push((pid, name.to_string(), visibility));
    }

    async fn deregister_tool(&self, pid: u64, name: &str) {
        self.registered
            .lock()
            .await
            .retain(|(p, n, _)| !(*p == pid && n == name));
    }

    async fn lookup_descriptor(&self, _name: &str) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Debug)]
pub struct TurnResult {
    pub text: String,
}

/// Output of a successful [`RuntimeExecutor::restore_from_snapshot`] call.
#[derive(Debug)]
pub struct RestoreResult {
    pub snapshot_name: String,
    pub agent_name: String,
    /// Request IDs that were in-flight at capture and need to be re-issued.
    pub reissued_requests: Vec<String>,
    /// Pipe IDs that were successfully reconnected.
    pub reconnected_pipes: Vec<String>,
    /// Pipe IDs whose target was gone; SIGPIPE will be delivered to these.
    pub sigpipe_pipes: Vec<String>,
}

pub struct RuntimeExecutor {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub session_id: String,
    /// ATP connection session ID — used for event_bus ownership routing.
    pub atp_session_id: String,
    pub token: CapabilityToken,
    pub pending_messages: Vec<String>,
    /// Tool management sub-struct (tool_list, budgets, HIL, registered_cat2).
    pub tools: ToolManager,
    #[allow(dead_code)]
    runtime_dir: PathBuf,
    // Mock LLM infrastructure (used by run_until_complete in tests)
    pub(self) llm_queue: Arc<std::sync::Mutex<Vec<LlmCompleteResponse>>>,
    pub(self) call_log: Arc<std::sync::Mutex<Vec<Vec<serde_json::Value>>>>,
    pub(self) fs_data: Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>,
    pub max_tool_chain_length: usize,
    // Optional kernel handles
    pub(self) kernel: Option<Arc<MockKernelHandle>>,
    pub(self) resource_handler: Option<Arc<KernelResourceHandler>>,
    pub(self) vfs: Option<Arc<VfsRouter>>,
    registry_ref: RegistryRef,
    /// Set to `true` by the signal listener when SIGPAUSE is received.
    pub paused: Arc<AtomicBool>,
    /// Set to `true` by the signal listener when SIGKILL or SIGSTOP is received.
    pub killed: Arc<AtomicBool>,
    /// Set to `true` when SIGSAVE is received; checked at turn start.
    pub snapshot_requested: Arc<AtomicBool>,
    /// Memory management sub-struct (memory_svc, memory_context, conversation_history).
    pub memory: MemoryManager,
    pub(self) event_bus: Option<Arc<AtpEventBus>>,
    pub(self) tracer: Option<Arc<Tracer>>,
    pub(self) invocation_store: Option<Arc<crate::invocation::InvocationStore>>,
    pub(self) invocation_id: String,
    pub(self) session_store: Option<Arc<crate::session::PersistentSessionStore>>,
    pub(self) snapshot_interval: Option<u32>,
    pub(self) tool_calls_since_last_snapshot: u32,

    // ── status tracking ───────────────────────────────────────────────────────
    pub spawned_at: chrono::DateTime<chrono::Utc>,
    pub context_used: u64,
    pub context_limit: u64,
    pub denied_tools: Vec<String>,
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    pub(self) last_signal_received: Arc<Mutex<Option<String>>>,
    pub pending_signal_count: Arc<AtomicU32>,
    pub(self) signal_tx: mpsc::Sender<Signal>,
    pub(self) signal_rx: Option<mpsc::Receiver<Signal>>,
}

pub(crate) enum RegistryRef {
    Mock(Arc<MockToolRegistry>),
    Real(Arc<ToolRegistry>),
}

impl RuntimeExecutor {
    /// Internal constructor accepting any `RegistryRef` variant.
    pub(crate) async fn spawn_with_registry_ref(
        params: SpawnParams,
        registry_ref: RegistryRef,
    ) -> Result<Self, AvixError> {
        let cat2_tools = compute_cat2_tools(&params.token, &params.spawned_by);
        let mut registered_cat2 = Vec::new();

        // Register Cat2 tools with the mock registry (no-op for Real registry).
        for (name, visibility) in &cat2_tools {
            match &registry_ref {
                RegistryRef::Mock(reg) => {
                    reg.register_tool(params.pid.as_u64(), name, visibility.clone())
                        .await;
                }
                RegistryRef::Real(_) => {
                    // Real registry manages tool registration via ipc.tool-add at agent spawn.
                }
            }
            registered_cat2.push(name.clone());
        }

        let (signal_tx, signal_rx) = mpsc::channel::<Signal>(64);

        let tools = ToolManager::new(registered_cat2);

        let mut executor = Self {
            pid: params.pid,
            agent_name: params.agent_name,
            goal: params.goal,
            spawned_by: params.spawned_by,
            session_id: params.session_id,
            atp_session_id: params.atp_session_id,
            token: params.token,
            pending_messages: Vec::new(),
            tools,
            llm_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            call_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            fs_data: Arc::new(std::sync::Mutex::new(HashMap::new())),
            max_tool_chain_length: 50,
            kernel: None,
            resource_handler: None,
            vfs: None,
            registry_ref,
            paused: Arc::new(AtomicBool::new(false)),
            killed: Arc::new(AtomicBool::new(false)),
            snapshot_requested: Arc::new(AtomicBool::new(false)),
            memory: MemoryManager::new(),
            runtime_dir: params.runtime_dir.clone(),
            event_bus: None,
            tracer: None,
            invocation_store: None,
            invocation_id: params.invocation_id.clone(),
            session_store: None,
            snapshot_interval: None,
            tool_calls_since_last_snapshot: 0,
            spawned_at: chrono::Utc::now(),
            context_used: 0,
            context_limit: params.context_limit,
            denied_tools: params.denied_tools,
            tokens_consumed: 0,
            tool_calls_total: 0,
            last_signal_received: Arc::new(Mutex::new(None)),
            pending_signal_count: Arc::new(AtomicU32::new(0)),
            signal_tx,
            signal_rx: Some(signal_rx),
        };

        executor.refresh_tool_list().await;

        Ok(executor)
    }

    pub async fn spawn_with_registry(
        params: SpawnParams,
        registry: Arc<MockToolRegistry>,
    ) -> Result<Self, AvixError> {
        Self::spawn_with_registry_ref(params, RegistryRef::Mock(registry)).await
    }

    /// Spawn with the real kernel `ToolRegistry` — used by `IpcExecutorFactory`.
    pub async fn spawn_with_real_registry(
        params: SpawnParams,
        registry: Arc<ToolRegistry>,
    ) -> Result<Self, AvixError> {
        Self::spawn_with_registry_ref(params, RegistryRef::Real(registry)).await
    }

    pub async fn spawn_with_registry_and_kernel(
        params: SpawnParams,
        registry: Arc<MockToolRegistry>,
        kernel: Arc<MockKernelHandle>,
    ) -> Result<Self, AvixError> {
        let mut executor = Self::spawn_with_registry(params, registry).await?;
        executor.kernel = Some(kernel);
        Ok(executor)
    }

    /// Attach an `AtpEventBus` so tool-call and tool-result events are published live.
    pub fn with_event_bus(mut self, bus: Arc<AtpEventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Attach a `Tracer` so LLM calls, tool calls, and exits are written to trace files.
    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attach a `KernelResourceHandler` for `cap/request-tool` and token renewal.
    pub fn with_resource_handler(mut self, handler: Arc<KernelResourceHandler>) -> Self {
        self.resource_handler = Some(handler);
        self
    }

    /// Attach a `MemFs` handle so `pipe/open` writes `/proc/<pid>/pipes/<pipeId>.yaml`.
    pub fn with_vfs(mut self, vfs: Arc<VfsRouter>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    /// Set the VFS caller context from this executor's token so `/tools/**` reads
    /// return `state: available/unavailable` per the agent's actual capability grants.
    /// Call this after `with_vfs()` in any async context.
    pub async fn init_vfs_caller(&self) {
        let Some(vfs) = &self.vfs else { return };
        let ctx = VfsCallerContext {
            username: self.spawned_by.clone(),
            crews: vec![], // capability grants (not crew membership) gate tool state
            is_admin: false,
            token: Some(self.token.clone()),
        };
        vfs.set_caller(Some(ctx)).await;
        tracing::debug!(
            pid = self.pid.as_u64(),
            spawned_by = %self.spawned_by,
            "VFS caller context set from agent token"
        );
    }

    /// Attach an `InvocationStore` so conversation history is flushed to disk on shutdown.
    pub fn with_invocation_store(
        mut self,
        store: Arc<crate::invocation::InvocationStore>,
        id: String,
    ) -> Self {
        self.invocation_store = Some(store);
        self.invocation_id = id;
        self
    }

    /// Attach a `SessionStore` so session status is updated on Idle transitions.
    pub fn with_session_store(
        mut self,
        store: Arc<crate::session::PersistentSessionStore>,
    ) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Set snapshot interval — persist_interim is called after every N tool calls.
    pub fn with_snapshot_interval(mut self, interval: u32) -> Self {
        self.snapshot_interval = Some(interval);
        self
    }

    /// Attach a `MemoryService` so SIGSTOP auto-logs the session to episodic memory.
    pub fn with_memory_svc(mut self, svc: Arc<crate::memory_svc::service::MemoryService>) -> Self {
        self.memory.memory_svc = Some(svc);
        self
    }

    // ── Memory delegates ──────────────────────────────────────────────────────

    /// Initialise the memory VFS tree for this agent (dirs under `/users/<owner>/memory/<agent>/`).
    pub async fn init_memory_tree(&self) {
        let vfs = match &self.vfs {
            Some(v) => Arc::clone(v),
            None => return,
        };
        let has_memory = self
            .token
            .granted_tools
            .iter()
            .any(|t| t.starts_with("memory/"));
        if !has_memory {
            return;
        }
        if let Err(e) = init_user_memory_tree(&vfs, &self.spawned_by, &self.agent_name).await {
            tracing::warn!(pid = self.pid.as_u64(), err = ?e, "memory tree init failed");
        }
    }

    /// Build and store the memory context block from existing VFS records.
    pub async fn init_memory_context(&mut self) {
        let vfs = self.vfs.clone();
        let spawned_by = self.spawned_by.clone();
        let agent_name = self.agent_name.clone();
        self.memory
            .init_memory_context(vfs.as_ref(), &spawned_by, &agent_name)
            .await;
    }

    /// Record a conversation message in the history (for session auto-log).
    pub fn push_conversation_message(&mut self, role: &str, content: &str) {
        self.memory.push_conversation_message(role, content);
    }

    // ── Tool delegates ────────────────────────────────────────────────────────

    /// Rebuild tool_list from Cat2 tools + Cat1 descriptors from registry, excluding removed tools.
    pub async fn refresh_tool_list(&mut self) {
        // Collect Cat1 tool names from the token (those not already registered as Cat2).
        let granted_tools: Vec<String> = self.token.granted_tools.clone();
        let registered_cat2: Vec<String> = self.tools.registered_cat2.clone();

        let mut cat1_descriptors = HashMap::new();
        for name in &granted_tools {
            if registered_cat2.contains(name) {
                continue; // Cat2 — descriptor comes from cat2_tool_descriptor
            }
            // Cat1 — look up descriptor from the real registry (mock always returns None).
            let desc = match &self.registry_ref {
                RegistryRef::Mock(_) => None,
                RegistryRef::Real(reg) => {
                    let reg = Arc::clone(reg);
                    reg.lookup(name).await.ok().map(|e| e.descriptor)
                }
            };
            match desc {
                Some(d) => {
                    tracing::debug!(
                        pid = self.pid.as_u64(),
                        tool = %name,
                        "Cat1 descriptor resolved from registry"
                    );
                    cat1_descriptors.insert(name.clone(), d);
                }
                None => {
                    tracing::debug!(
                        pid = self.pid.as_u64(),
                        tool = %name,
                        "Cat1 tool granted but not yet in registry — omitted from tool list"
                    );
                }
            }
        }

        let cat1_count = cat1_descriptors.len();
        self.tools.cat1_descriptors = cat1_descriptors;

        let token = self.token.clone();
        let spawned_by = self.spawned_by.clone();
        self.tools.refresh_tool_list(&token, &spawned_by);

        tracing::debug!(
            pid = self.pid.as_u64(),
            cat2_count = self.tools.registered_cat2.len(),
            cat1_count,
            total = self.tools.tool_list.len(),
            "tool list refreshed"
        );
    }

    pub fn current_tool_list(&self) -> Vec<serde_json::Value> {
        self.tools.current_tool_list()
    }

    /// Returns true if this tool is a registered Category 2 tool for this agent.
    pub fn is_cat2_tool(&self, name: &str) -> bool {
        self.tools.is_cat2_tool(name)
    }

    pub async fn handle_tool_changed(&mut self, op: &str, tool_name: &str, _reason: &str) {
        self.tools.handle_tool_changed(op, tool_name);
    }

    /// Set a per-tool call budget.
    pub fn set_tool_budget(&mut self, tool: &str, n: u32) {
        self.tools.set_tool_budget(tool, n);
    }

    /// Register a tool that requires HIL approval before dispatch.
    pub fn require_hil_for(&mut self, tool: &str) {
        self.tools.require_hil_for(tool);
    }

    // ── System prompt ─────────────────────────────────────────────────────────

    pub fn build_system_prompt_str(&self) -> String {
        let tool_list = self.current_tool_list();
        let base = build_system_prompt(
            self.pid.as_u64(),
            &self.agent_name,
            &self.goal,
            &self.spawned_by,
            &self.session_id,
            self.max_tool_chain_length,
            &self
                .tools
                .registered_cat2
                .iter()
                .filter_map(|name| {
                    self.tools
                        .tool_budgets
                        .remaining(name)
                        .map(|n| (name.clone(), n))
                })
                .collect::<HashMap<String, u32>>(),
            &self.pending_messages,
            &tool_list,
        );
        if let Some(ref ctx) = self.memory.memory_context {
            format!("{ctx}\n\n{base}")
        } else {
            base
        }
    }

    /// Accessor for the system prompt (for tests and context inspection).
    pub fn system_prompt(&self) -> String {
        self.build_system_prompt_str()
    }

    /// Public accessor kept for backward compat with existing tests.
    pub fn build_system_prompt(&self) -> String {
        self.build_system_prompt_str()
    }

    pub fn inject_pending_message(&mut self, msg: String) {
        self.pending_messages.push(msg);
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Return the current goal (used in tests and restore verification).
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// Return the current capability token.
    pub fn token(&self) -> &CapabilityToken {
        &self.token
    }

    /// Return a clone of the signal sender for external signal delivery.
    pub fn signal_sender(&self) -> mpsc::Sender<Signal> {
        self.signal_tx.clone()
    }

    // ── Mock / test helpers ───────────────────────────────────────────────────

    pub fn push_llm_response(&self, resp: LlmCompleteResponse) {
        self.llm_queue.lock().unwrap().push(resp);
    }

    pub fn llm_call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }

    pub fn call_messages(&self, idx: usize) -> Vec<serde_json::Value> {
        self.call_log
            .lock()
            .unwrap()
            .get(idx)
            .cloned()
            .unwrap_or_default()
    }

    /// Set the token's expiry to `now + d`. Used in tests to simulate near-expiry tokens.
    pub fn set_token_expiry_in(&mut self, d: Duration) {
        self.token.expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(d).unwrap_or(chrono::Duration::hours(1));
    }

    pub fn on_fs_read(&self, path: &str, content: &[u8]) {
        self.fs_data
            .lock()
            .unwrap()
            .insert(path.to_string(), content.to_vec());
    }

    pub fn set_max_tool_chain_length(&mut self, max: usize) {
        self.max_tool_chain_length = max;
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    pub async fn shutdown(&mut self) {
        self.shutdown_with_status(crate::invocation::InvocationStatus::Completed, None)
            .await;
    }

    /// Transition this executor to idle, persisting invocation and session state.
    ///
    /// Called after a successful turn when the agent is waiting for the next message.
    /// Unlike `shutdown_with_status`, this does NOT deregister Cat2 tools — the executor
    /// stays alive and can accept another goal via `wait_for_next_goal`.
    pub async fn idle(&mut self) {
        tracing::debug!(pid = self.pid.as_u64(), "executor transitioning to idle");

        if !self.invocation_id.is_empty() {
            if let Some(store) = &self.invocation_store {
                let _ = store
                    .update_status(
                        &self.invocation_id,
                        crate::invocation::InvocationStatus::Idle,
                    )
                    .await;
            }
        }

        if !self.session_id.is_empty() {
            if let Some(store) = &self.session_store {
                if let Ok(Some(mut session)) = store
                    .get(&uuid::Uuid::parse_str(&self.session_id).unwrap_or_default())
                    .await
                {
                    session.mark_idle();
                    let _ = store.update(&session).await;
                }
            }
        }
    }

    /// Block until a `SIGSTART` signal arrives carrying the next goal string.
    ///
    /// Handles `SIGPAUSE`/`SIGRESUME` while waiting. Returns `Some(goal)` on `SIGSTART`,
    /// `None` if the executor is killed or the signal channel is closed.
    pub async fn wait_for_next_goal(&mut self) -> Option<String> {
        use std::sync::atomic::Ordering;
        // Take ownership so we can freely borrow `self` inside the loop.
        let mut signal_rx = self.signal_rx.take()?;
        loop {
            match signal_rx.recv().await {
                Some(sig) => {
                    match &sig.kind {
                        crate::signal::kind::SignalKind::Start => {
                            let goal = sig.payload["goal"].as_str().unwrap_or("").to_string();
                            tracing::info!(
                                pid = self.pid.as_u64(),
                                "SIGSTART received; resuming executor with new goal"
                            );
                            self.signal_rx = Some(signal_rx);
                            return Some(goal);
                        }
                        crate::signal::kind::SignalKind::Kill
                        | crate::signal::kind::SignalKind::Stop => {
                            self.killed.store(true, Ordering::Release);
                            tracing::info!(
                                pid = self.pid.as_u64(),
                                signal = ?sig.kind,
                                "executor killed while idle"
                            );
                            self.signal_rx = Some(signal_rx);
                            return None;
                        }
                        _ => {
                            self.handle_signal_between_turns(&sig).await;
                            if self.killed.load(Ordering::Acquire) {
                                self.signal_rx = Some(signal_rx);
                                return None;
                            }
                        }
                    }
                }
                None => {
                    tracing::debug!(
                        pid = self.pid.as_u64(),
                        "signal channel closed while waiting for next goal"
                    );
                    self.signal_rx = Some(signal_rx);
                    return None;
                }
            }
        }
    }

    /// Shutdown the executor, deregistering tools and flushing invocation history.
    pub async fn shutdown_with_status(
        &mut self,
        status: crate::invocation::InvocationStatus,
        exit_reason: Option<String>,
    ) {
        // 1. Deregister Category 2 tools.
        match &self.registry_ref {
            RegistryRef::Mock(reg) => {
                for name in self.tools.registered_cat2.clone() {
                    reg.deregister_tool(self.pid.as_u64(), &name).await;
                }
                self.tools.registered_cat2.clear();
            }
            RegistryRef::Real(_) => {
                // Real registry deregistration is handled by ipc.tool-remove at the kernel level.
                self.tools.registered_cat2.clear();
            }
        }

        // 2. Handle Idle transition
        if exit_reason.as_deref() == Some("waiting_for_input") {
            if !self.invocation_id.is_empty() {
                if let Some(store) = &self.invocation_store {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Idle,
                        )
                        .await;
                }
            }
            if !self.session_id.is_empty() {
                if let Some(store) = &self.session_store {
                    if let Ok(Some(mut session)) = store
                        .get(&uuid::Uuid::parse_str(&self.session_id).unwrap_or_default())
                        .await
                    {
                        session.mark_idle();
                        let _ = store.update(&session).await;
                    }
                }
            }
            return;
        }

        // 3. Flush conversation history and finalize invocation record.
        if !self.invocation_id.is_empty() {
            if let Some(store) = &self.invocation_store {
                // Append exit reason as a final system message so users can see
                // why the agent stopped (e.g. "exceeded max tool chain limit").
                if let Some(ref reason) = exit_reason {
                    use crate::invocation::conversation::{ConversationEntry, Role};
                    self.memory.conversation_history.push(
                        ConversationEntry::from_role_content(
                            Role::System,
                            format!("[Agent stopped: {}]", reason),
                        ),
                    );
                }
                let _ = store
                    .write_conversation_structured(
                        &self.invocation_id,
                        &self.spawned_by,
                        &self.agent_name,
                        &self.memory.conversation_history,
                    )
                    .await;
                let _ = store
                    .finalize(
                        &self.invocation_id,
                        status,
                        chrono::Utc::now(),
                        self.tokens_consumed,
                        self.tool_calls_total,
                        exit_reason,
                    )
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::MockKernelHandle;

    fn make_params(pid_val: u64, caps: &[&str]) -> SpawnParams {
        SpawnParams {
            pid: Pid::from_u64(pid_val),
            agent_name: "test-agent".into(),
            goal: "test goal".into(),
            spawned_by: "kernel".into(),
            session_id: "sess-test".into(),
            atp_session_id: String::new(),
            token: CapabilityToken::test_token(caps),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
            invocation_id: String::new(),
        }
    }

    async fn make_executor(pid_val: u64, caps: &[&str]) -> RuntimeExecutor {
        let registry = Arc::new(MockToolRegistry::new());
        RuntimeExecutor::spawn_with_registry(make_params(pid_val, caps), registry)
            .await
            .unwrap()
    }

    // GAP 3 tests
    #[tokio::test]
    async fn test_tool_list_populated_at_spawn() {
        let executor = make_executor(200, &[]).await;
        assert!(
            !executor.tools.tool_list.is_empty(),
            "tool_list should be non-empty after spawn"
        );
    }

    #[tokio::test]
    async fn test_tool_list_excludes_removed() {
        let mut executor = make_executor(201, &[]).await;
        executor
            .handle_tool_changed("removed", "cap/list", "")
            .await;
        executor.refresh_tool_list().await;
        let names: Vec<_> = executor
            .tools
            .tool_list
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(
            !names.contains(&"cap/list"),
            "cap/list should be excluded after removal"
        );
    }

    // Dispatch/run tests live in runtime_executor/dispatch_manager.rs

    #[tokio::test]
    async fn test_set_max_tool_chain_length() {
        let mut executor = make_executor(230, &[]).await;
        assert_eq!(executor.max_tool_chain_length, 50);
        executor.set_max_tool_chain_length(10);
        assert_eq!(executor.max_tool_chain_length, 10);
    }

    #[tokio::test]
    async fn test_set_tool_budget() {
        let mut executor = make_executor(231, &["fs/read"]).await;
        executor.set_tool_budget("fs/read", 5);
        assert_eq!(executor.tools.tool_budgets.remaining("fs/read"), Some(5));
    }

    #[tokio::test]
    async fn test_inject_pending_message_accumulates() {
        let mut executor = make_executor(233, &[]).await;
        executor.inject_pending_message("msg-1".into());
        executor.inject_pending_message("msg-2".into());
        executor.inject_pending_message("msg-3".into());
        assert_eq!(executor.pending_messages.len(), 3);
        assert_eq!(executor.pending_messages[0], "msg-1");
        assert_eq!(executor.pending_messages[2], "msg-3");
    }

    #[tokio::test]
    async fn test_is_cat2_tool() {
        let executor = make_executor(
            251,
            &[
                "agent/spawn",
                "agent/kill",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        )
        .await;
        assert!(executor.is_cat2_tool("cap/list"));
        assert!(executor.is_cat2_tool("agent/spawn"));
        assert!(!executor.is_cat2_tool("fs/read"));
        assert!(!executor.is_cat2_tool("llm/complete"));
    }

    #[tokio::test]
    async fn test_spawn_with_registry_and_kernel() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        let params = make_params(240, &["cap/list"]);
        let executor = RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, kernel)
            .await
            .unwrap();
        assert!(!executor.tools.tool_list.is_empty());
    }

    #[tokio::test]
    async fn test_set_token_expiry_in_and_on_fs_read() {
        let mut executor = make_executor(241, &[]).await;
        executor.set_token_expiry_in(Duration::from_secs(300));
        executor.on_fs_read("/tmp/test.txt", b"hello world");
    }

    // T-REX-20: shutdown_with_status(Completed) finalizes invocation
    #[tokio::test]
    async fn test_shutdown_with_status_completed_finalizes_invocation() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let rec = InvocationRecord::new(
            "inv-rex-20".into(),
            "test-agent".into(),
            "kernel".into(),
            200,
            "test goal".into(),
            "sess-1".into(),
        );
        store.create(&rec).await.unwrap();

        let mut executor = make_executor(200, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-20".into();
        use crate::invocation::conversation::{ConversationEntry, Role};
        executor.memory.conversation_history = vec![
            ConversationEntry::from_role_content(Role::User, "hello"),
            ConversationEntry::from_role_content(Role::Assistant, "hi"),
        ];
        executor.tokens_consumed = 1234;
        executor.tool_calls_total = 5;

        executor
            .shutdown_with_status(InvocationStatus::Completed, None)
            .await;

        let loaded = store.get("inv-rex-20").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Completed);
        assert!(loaded.ended_at.is_some());
        assert_eq!(loaded.tokens_consumed, 1234);
        assert_eq!(loaded.tool_calls_total, 5);
    }

    // T-REX-21: shutdown_with_status(Killed)
    #[tokio::test]
    async fn test_shutdown_with_status_killed() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let rec = InvocationRecord::new(
            "inv-rex-21".into(),
            "test-agent".into(),
            "kernel".into(),
            201,
            "test goal".into(),
            "sess-1".into(),
        );
        store.create(&rec).await.unwrap();

        let mut executor = make_executor(201, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-21".into();

        executor
            .shutdown_with_status(InvocationStatus::Killed, Some("killed".into()))
            .await;

        let loaded = store.get("inv-rex-21").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Killed);
        assert_eq!(loaded.exit_reason.as_deref(), Some("killed"));
    }

    // T-REX-22: shutdown_with_status(Failed)
    #[tokio::test]
    async fn test_shutdown_with_status_failed() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let rec = InvocationRecord::new(
            "inv-rex-22".into(),
            "test-agent".into(),
            "kernel".into(),
            202,
            "test goal".into(),
            "sess-1".into(),
        );
        store.create(&rec).await.unwrap();

        let mut executor = make_executor(202, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-22".into();

        executor
            .shutdown_with_status(InvocationStatus::Failed, Some("token expired".into()))
            .await;

        let loaded = store.get("inv-rex-22").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Failed);
        assert_eq!(loaded.exit_reason.as_deref(), Some("token expired"));
    }

    // T-REX-23: executor without store — shutdown_with_status doesn't panic
    #[tokio::test]
    async fn test_shutdown_with_status_no_store_no_panic() {
        use crate::invocation::InvocationStatus;
        let mut executor = make_executor(203, &[]).await;
        executor
            .shutdown_with_status(InvocationStatus::Completed, None)
            .await;
    }

    // T-REX-24: 3-message conversation produces 3-line JSONL
    #[tokio::test]
    async fn test_shutdown_flushes_3_message_conversation_as_jsonl() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use crate::memfs::local_provider::LocalProvider;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap()
                .with_local(provider),
        );
        let rec = InvocationRecord::new(
            "inv-rex-24".into(),
            "test-agent".into(),
            "kernel".into(),
            204,
            "test goal".into(),
            "sess-1".into(),
        );
        store.create(&rec).await.unwrap();

        let mut executor = make_executor(204, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-24".into();
        use crate::invocation::conversation::{ConversationEntry, Role};
        executor.memory.conversation_history = vec![
            ConversationEntry::from_role_content(Role::User, "msg1"),
            ConversationEntry::from_role_content(Role::Assistant, "msg2"),
            ConversationEntry::from_role_content(Role::User, "msg3"),
        ];

        executor
            .shutdown_with_status(InvocationStatus::Completed, None)
            .await;

        let path = dir
            .path()
            .join("kernel/agents/test-agent/invocations/inv-rex-24/conversation.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content.lines().count(), 3);
    }

    // T-REX-30: SIGPAUSE updates invocation store to Paused
    #[tokio::test]
    async fn sigpause_updates_invocation_store_to_paused() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let record = InvocationRecord::new(
            "inv-rex-30".into(),
            "test-agent".into(),
            "kernel".into(),
            230,
            "goal".into(),
            "sess-1".into(),
        );
        store.create(&record).await.unwrap();

        let mut executor = make_executor(230, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-30".into();

        executor.deliver_signal("SIGPAUSE").await;

        let loaded = store.get("inv-rex-30").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Paused);
    }

    // T-REX-31: SIGRESUME updates invocation store back to Running
    #[tokio::test]
    async fn sigresume_updates_invocation_store_to_running() {
        use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let store = Arc::new(
            InvocationStore::open(dir.path().join("inv.redb"))
                .await
                .unwrap(),
        );
        let record = InvocationRecord::new(
            "inv-rex-31".into(),
            "test-agent".into(),
            "kernel".into(),
            231,
            "goal".into(),
            "sess-1".into(),
        );
        store.create(&record).await.unwrap();

        let mut executor = make_executor(231, &[]).await;
        executor.invocation_store = Some(Arc::clone(&store));
        executor.invocation_id = "inv-rex-31".into();

        executor.deliver_signal("SIGPAUSE").await;
        executor.deliver_signal("SIGRESUME").await;

        let loaded = store.get("inv-rex-31").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Running);
    }

    // T-REX-32: SIGPAUSE without invocation store does not panic
    #[tokio::test]
    async fn sigpause_without_invocation_store_does_not_panic() {
        let executor = make_executor(232, &[]).await;
        executor.deliver_signal("SIGPAUSE").await;
    }

    // GAP-A: lookup_descriptor on real registry returns stored descriptor
    #[tokio::test]
    async fn test_real_registry_lookup_descriptor_returns_descriptor() {
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::types::tool::ToolName;

        let reg = Arc::new(ToolRegistry::new());
        let descriptor = serde_json::json!({
            "name": "fs/read",
            "description": "Read a file from the virtual filesystem",
            "input_schema": { "type": "object", "properties": {}, "required": [] }
        });
        reg.add(
            "fs.svc",
            vec![ToolEntry::new(
                ToolName::parse("fs/read").unwrap(),
                "fs.svc".into(),
                ToolState::Available,
                ToolVisibility::All,
                descriptor.clone(),
            )],
        )
        .await
        .unwrap();

        // lookup_descriptor via the ToolRegistryHandle impl
        let result = reg.lookup_descriptor("fs/read").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap()["description"], descriptor["description"]);
    }

    // GAP-A: executor with real registry and token granting fs/read includes fs/read in tool_list
    #[tokio::test]
    async fn test_cat1_descriptors_merged_into_tool_list_when_registry_has_entry() {
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::types::tool::ToolName;

        let reg = Arc::new(ToolRegistry::new());
        let descriptor = serde_json::json!({
            "name": "fs/read",
            "description": "Read a file",
            "input_schema": { "type": "object", "properties": {}, "required": [] }
        });
        reg.add(
            "fs.svc",
            vec![ToolEntry::new(
                ToolName::parse("fs/read").unwrap(),
                "fs.svc".into(),
                ToolState::Available,
                ToolVisibility::All,
                descriptor,
            )],
        )
        .await
        .unwrap();

        let params = make_params(900, &["fs/read"]);
        let executor = RuntimeExecutor::spawn_with_real_registry(params, Arc::clone(&reg))
            .await
            .unwrap();

        let tool_list = executor.current_tool_list();
        let names: Vec<_> = tool_list
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"fs/read"), "fs/read should appear in tool_list when registry holds it");
    }
}
