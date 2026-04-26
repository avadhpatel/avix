use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn, instrument};

use crate::executor::factory::AgentExecutorFactory;
use crate::executor::runtime_executor::RuntimeExecutor;
use crate::tool_registry::ToolRegistry;
use crate::executor::spawn::SpawnParams;
use crate::gateway::event_bus::AtpEventBus;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::kernel::approval_token::ApprovalTokenStore;
use crate::kernel::{HilManager, KernelResourceHandler};
use crate::llm_client::IpcLlmClient;
use crate::memfs::vfs::MemFs;
use crate::memfs::VfsRouter;
use crate::process::entry::ProcessStatus;
use crate::process::table::ProcessTable;
use crate::session::PersistentSessionStore;
use crate::signal::{bus::SignalBus, SignalChannelRegistry};
use crate::trace::Tracer;

/// Concrete `AgentExecutorFactory` wired into the kernel bootstrap.
///
/// For each `launch()` call it:
///   1. Resolves the `llm.svc` socket path from `AVIX_LLM_SOCK` env var or
///      `params.runtime_dir/llm.sock` (derived at launch time, not stored at construction).
///   2. Creates an `IpcLlmClient` pointed at that socket.
///   3. Builds a `RuntimeExecutor` via `spawn_with_registry`.
///   4. Runs `run_with_client` inside a detached tokio task.
///   5. Publishes `agent_output`, `agent_status`, and `agent_exit` ATP events.
///   6. Updates the process table status to `Stopped` (success) or `Crashed` (error).
///   7. Returns the task's `AbortHandle` so `kernel/proc/kill` can stop it.
pub struct IpcExecutorFactory {
    /// Shared process table — used to update agent status on exit.
    process_table: Arc<ProcessTable>,
    /// Event bus — used to publish agent output/status/exit events to ATP clients.
    event_bus: Arc<AtpEventBus>,
    /// Tracer — records agent spawn, LLM calls, tool calls, and exit.
    tracer: Arc<Tracer>,
    /// Invocation store — persists `InvocationRecord` YAML + JSONL conversation to disk.
    invocation_store: Arc<InvocationStore>,
    /// Session store — persists `SessionRecord` state across turns.
    session_store: Arc<PersistentSessionStore>,
    /// In-process signal channel registry — used to wire signals to running executor tasks.
    signal_channels: SignalChannelRegistry,
    /// Real kernel tool registry — injected in phase3 via `set_tool_registry`.
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,
    /// Shared VFS — attached to each executor so `/tools/**` reads reflect per-agent state.
    vfs: Option<Arc<VfsRouter>>,
    /// HMAC-based capability token validator for `cap/request-tool`.
    resource_handler: Arc<KernelResourceHandler>,
    /// HIL manager — drives the human-in-the-loop approval flow.
    hil_manager: Arc<HilManager>,
    /// Signal bus — used to send SIGPAUSE/SIGRESUME during HIL.
    signal_bus: Arc<SignalBus>,
}

impl IpcExecutorFactory {
    pub fn new(
        process_table: Arc<ProcessTable>,
        event_bus: Arc<AtpEventBus>,
        invocation_store: Arc<InvocationStore>,
        session_store: Arc<PersistentSessionStore>,
        hmac_key: Vec<u8>,
    ) -> Self {
        let signal_bus = Arc::new(SignalBus::new());
        let approval_store = Arc::new(ApprovalTokenStore::new());
        let hil_vfs = Arc::new(MemFs::new());
        let hil_manager = HilManager::new(
            Arc::clone(&approval_store),
            Arc::clone(&event_bus),
            hil_vfs,
            Arc::clone(&signal_bus),
            600,
        );
        Self {
            process_table,
            event_bus,
            tracer: Tracer::noop(),
            invocation_store,
            session_store,
            signal_channels: SignalChannelRegistry::new(),
            tool_registry: Arc::new(Mutex::new(None)),
            vfs: None,
            resource_handler: Arc::new(KernelResourceHandler::new(hmac_key)),
            hil_manager,
            signal_bus,
        }
    }

    /// Wire in the shared `VfsRouter` so each launched executor gets per-agent tool state.
    pub fn with_vfs(mut self, vfs: Arc<VfsRouter>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    /// Wire in the real `ToolRegistry` after phase3 construction.
    #[instrument(skip(self, registry))]
    pub async fn set_tool_registry(&self, registry: Arc<ToolRegistry>) {
        *self.tool_registry.lock().await = Some(registry);
        info!("real ToolRegistry injected into IpcExecutorFactory");
    }

    pub fn with_tracer(mut self, tracer: Arc<Tracer>) -> Self {
        self.tracer = tracer;
        self
    }

    pub fn with_signal_channels(mut self, channels: SignalChannelRegistry) -> Self {
        self.signal_channels = channels;
        self
    }
}

impl AgentExecutorFactory for IpcExecutorFactory {
    #[instrument(skip_all)]
    fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle {
        // Derive the llm.svc socket from the agent's runtime_dir.  By the time
        // an agent is launched phase-3 will have started llm.svc at this path.
        let llm_sock = params.runtime_dir.join("llm.sock");
        let process_table = Arc::clone(&self.process_table);
        let event_bus = Arc::clone(&self.event_bus);
        let tracer = Arc::clone(&self.tracer);
        let invocation_store = Arc::clone(&self.invocation_store);
        let session_store = Arc::clone(&self.session_store);

        let pid = params.pid;
        let agent_name = params.agent_name.clone();
        let goal = params.goal.clone();
        let agent_session_id = params.session_id.clone();
        let atp_session_id = params.atp_session_id.clone();
        let invocation_id = params.invocation_id.clone();
        let restore_from_pid = params.restore_from_pid;
        let spawned_by = params.spawned_by.clone();
        let signal_channels = self.signal_channels.clone();
        let tool_registry_handle = Arc::clone(&self.tool_registry);
        let vfs_handle = self.vfs.clone();
        let resource_handler = Arc::clone(&self.resource_handler);
        let hil_manager = Arc::clone(&self.hil_manager);
        let signal_bus = Arc::clone(&self.signal_bus);

        let handle = tokio::spawn(async move {
            let is_restore = restore_from_pid.is_some();

            if is_restore {
                info!(
                    pid = pid.as_u64(),
                    agent = %agent_name,
                    session_id = %agent_session_id,
                    "restoring interrupted agent"
                );
            } else {
                tracer.agent_spawn(pid.as_u64(), &agent_name, &goal, &agent_session_id);
                event_bus.agent_spawned(&atp_session_id, pid.as_u64(), &agent_name, &goal, &agent_session_id);
            }

            let llm_client = IpcLlmClient::new(
                llm_sock.to_string_lossy().to_string(),
                pid.as_u64(),
                agent_session_id.clone(),
            );

            // Resolve real ToolRegistry if available, otherwise fall back to mock.
            let registry_opt = tool_registry_handle.lock().await.clone();
            let spawn_result = match registry_opt {
                Some(registry) => {
                    tracing::debug!(pid = pid.as_u64(), "spawning executor with real ToolRegistry");
                    RuntimeExecutor::spawn_with_real_registry(params, registry).await
                }
                None => {
                    warn!(pid = pid.as_u64(), "real ToolRegistry not yet available — spawning executor with mock registry");
                    use crate::executor::runtime_executor::MockToolRegistry;
                    let mock = Arc::new(MockToolRegistry::new());
                    RuntimeExecutor::spawn_with_registry(params, mock).await
                }
            };

            let mut executor = match spawn_result {
                Ok(e) => e,
                Err(err) => {
                    warn!(pid = pid.as_u64(), error = %err, "executor spawn failed");
                    tracer.agent_exit(pid.as_u64(), "crashed", Some("spawn failed"));
                    let _ = process_table.set_status(pid, ProcessStatus::Crashed).await;
                    event_bus.agent_status(&atp_session_id, pid.as_u64(), "crashed");
                    event_bus.agent_exit(&atp_session_id, pid.as_u64(), 1);
                    return;
                }
            };

            // Register the in-process signal channel so SignalHandler can reach this executor.
            signal_channels.register(pid, executor.signal_sender()).await;

            // Wire the event bus, tracer, persistence stores, VFS, and HIL infrastructure.
            executor = executor.with_event_bus(Arc::clone(&event_bus));
            executor = executor.with_tracer(Arc::clone(&tracer));
            executor = executor.with_invocation_store(Arc::clone(&invocation_store), invocation_id);
            executor = executor.with_session_store(session_store);
            executor = executor.with_resource_handler(resource_handler);
            executor = executor.with_hil_manager(hil_manager);
            executor = executor.with_signal_bus(signal_bus);
            if let Some(vfs) = vfs_handle {
                executor = executor.with_vfs(Arc::clone(&vfs));
                executor.init_vfs_caller().await;
            }

            // ── Restore mode ─────────────────────────────────────────────────
            // Load persisted conversation from the previous run's JSONL, persist
            // idle state with the restored history, then go straight to waiting.
            // The next SIGSTART from the user triggers the first new LLM turn.
            if let Some(old_pid) = restore_from_pid {
                let history = invocation_store
                    .read_conversation(&agent_session_id, old_pid, &spawned_by)
                    .await
                    .unwrap_or_default();
                let history_len = history.len();
                executor.memory.conversation_history = history;
                info!(
                    pid = pid.as_u64(),
                    old_pid,
                    history_entries = history_len,
                    "conversation history restored"
                );

                // Persist Idle status and flush conversation so the record is consistent.
                executor.idle().await;

                event_bus.agent_status(&atp_session_id, pid.as_u64(), "waiting");
                let _ = process_table.set_status(pid, ProcessStatus::Waiting).await;
                info!(pid = pid.as_u64(), "restored agent waiting for next goal (SIGSTART)");

                // Fall through to the turn loop below (first iteration skips run_with_client
                // and goes directly to wait_for_next_goal).
                match executor.wait_for_next_goal().await {
                    Some(next_goal) => {
                        info!(pid = pid.as_u64(), "restored agent received first goal; starting turn");
                        // Continue into the shared turn loop with this goal.
                        run_turn_loop(
                            executor,
                            next_goal,
                            &llm_client,
                            pid,
                            &atp_session_id,
                            &event_bus,
                            &tracer,
                            &process_table,
                        )
                        .await;
                    }
                    None => {
                        info!(pid = pid.as_u64(), "restored agent killed before receiving first goal");
                    }
                }

                signal_channels.unregister(pid).await;
                return;
            }

            info!(pid = pid.as_u64(), "executor started");

            // ── Normal turn loop ─────────────────────────────────────────────
            run_turn_loop(
                executor,
                goal,
                &llm_client,
                pid,
                &atp_session_id,
                &event_bus,
                &tracer,
                &process_table,
            )
            .await;

            signal_channels.unregister(pid).await;
        });

        handle.abort_handle()
    }
}

/// Shared turn loop used by both normal spawn and post-restore continuation.
async fn run_turn_loop(
    mut executor: RuntimeExecutor,
    initial_goal: String,
    llm_client: &IpcLlmClient,
    pid: crate::types::Pid,
    atp_session_id: &str,
    event_bus: &Arc<crate::gateway::event_bus::AtpEventBus>,
    tracer: &Arc<crate::trace::Tracer>,
    process_table: &Arc<crate::process::table::ProcessTable>,
) {
    let mut current_goal = initial_goal;
    loop {
        event_bus.agent_status(atp_session_id, pid.as_u64(), "running");
        let _ = process_table.set_status(pid, ProcessStatus::Running).await;

        match executor.run_with_client(&current_goal, llm_client).await {
            Ok(_result) => {
                info!(pid = pid.as_u64(), "executor turn finished; transitioning to idle");
                executor.idle().await;
                event_bus.agent_status(atp_session_id, pid.as_u64(), "waiting");
                let _ = process_table.set_status(pid, ProcessStatus::Waiting).await;
                info!(pid = pid.as_u64(), "executor waiting for next goal (SIGSTART)");
                match executor.wait_for_next_goal().await {
                    Some(next_goal) => {
                        info!(pid = pid.as_u64(), "received next goal; resuming");
                        executor.record_next_goal(&next_goal).await;
                        current_goal = next_goal;
                    }
                    None => {
                        info!(pid = pid.as_u64(), "executor shutting down after idle wait");
                        break;
                    }
                }
            }
            Err(err) => {
                warn!(pid = pid.as_u64(), error = %err, "executor crashed");
                executor
                    .shutdown_with_status(InvocationStatus::Failed, Some(err.to_string()))
                    .await;
                tracer.agent_exit(pid.as_u64(), "crashed", Some(&err.to_string()));
                event_bus.agent_status(atp_session_id, pid.as_u64(), "crashed");
                event_bus.agent_exit(atp_session_id, pid.as_u64(), 1);
                let _ = process_table
                    .set_status(crate::types::Pid::from_u64(pid.as_u64()), ProcessStatus::Crashed)
                    .await;
                break;
            }
        }
    }
}
