use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::executor::factory::AgentExecutorFactory;
use crate::executor::runtime_executor::RuntimeExecutor;
use crate::tool_registry::ToolRegistry;
use crate::executor::spawn::SpawnParams;
use crate::gateway::event_bus::AtpEventBus;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::llm_client::IpcLlmClient;
use crate::process::entry::ProcessStatus;
use crate::process::table::ProcessTable;
use crate::session::PersistentSessionStore;
use crate::signal::SignalChannelRegistry;
use crate::trace::Tracer;
use crate::types::Pid;

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
}

impl IpcExecutorFactory {
    pub fn new(
        process_table: Arc<ProcessTable>,
        event_bus: Arc<AtpEventBus>,
        invocation_store: Arc<InvocationStore>,
        session_store: Arc<PersistentSessionStore>,
    ) -> Self {
        Self {
            process_table,
            event_bus,
            tracer: Tracer::noop(),
            invocation_store,
            session_store,
            signal_channels: SignalChannelRegistry::new(),
            tool_registry: Arc::new(Mutex::new(None)),
        }
    }

    /// Wire in the real `ToolRegistry` after phase3 construction.
    pub async fn set_tool_registry(&self, registry: Arc<ToolRegistry>) {
        *self.tool_registry.lock().await = Some(registry);
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
        let session_id = params.session_id.clone();
        let invocation_id = params.invocation_id.clone();
        let signal_channels = self.signal_channels.clone();
        let tool_registry_handle = Arc::clone(&self.tool_registry);

        let handle = tokio::spawn(async move {
            tracer.agent_spawn(pid.as_u64(), &agent_name, &goal, &session_id);

            let llm_client = IpcLlmClient::new(
                llm_sock.to_string_lossy().to_string(),
                pid.as_u64(),
                session_id.clone(),
            );

            // Resolve real ToolRegistry if available, otherwise fall back to mock.
            let registry_opt = tool_registry_handle.lock().await.clone();
            let spawn_result = match registry_opt {
                Some(registry) => {
                    RuntimeExecutor::spawn_with_real_registry(params, registry).await
                }
                None => {
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
                    event_bus.agent_status(&session_id, pid.as_u64(), "crashed");
                    event_bus.agent_exit(&session_id, pid.as_u64(), 1);
                    return;
                }
            };

            // Register the in-process signal channel so SignalHandler can reach this executor.
            signal_channels.register(pid, executor.signal_sender()).await;

            // Wire the event bus, tracer, and persistence stores.
            executor = executor.with_event_bus(Arc::clone(&event_bus));
            executor = executor.with_tracer(Arc::clone(&tracer));
            executor = executor.with_invocation_store(invocation_store, invocation_id);
            executor = executor.with_session_store(session_store);

            info!(pid = pid.as_u64(), "executor started");

            let mut current_goal = goal;

            // ── Turn loop ────────────────────────────────────────────────────
            // Each iteration processes one user message. After a successful
            // turn the executor idles and waits for SIGSTART (next message).
            // The loop exits on SIGKILL, SIGSTOP, or an unrecoverable error.
            loop {
                event_bus.agent_status(&session_id, pid.as_u64(), "running");

                match executor.run_with_client(&current_goal, &llm_client).await {
                    Ok(_result) => {
                        info!(pid = pid.as_u64(), "executor turn finished; transitioning to idle");

                        // Persist idle state — invocation + session status → Idle.
                        executor.idle().await;

                        event_bus.agent_status(&session_id, pid.as_u64(), "waiting");
                        let _ = process_table.set_status(pid, ProcessStatus::Waiting).await;

                        info!(pid = pid.as_u64(), "executor waiting for next goal (SIGSTART)");

                        // Block until a new goal arrives via SIGSTART or a kill signal.
                        match executor.wait_for_next_goal().await {
                            Some(next_goal) => {
                                info!(pid = pid.as_u64(), "received next goal; resuming");
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
                            .shutdown_with_status(
                                InvocationStatus::Failed,
                                Some(err.to_string()),
                            )
                            .await;
                        tracer.agent_exit(pid.as_u64(), "crashed", Some(&err.to_string()));
                        event_bus.agent_status(&session_id, pid.as_u64(), "crashed");
                        event_bus.agent_exit(&session_id, pid.as_u64(), 1);
                        let _ = process_table
                            .set_status(Pid::from_u64(pid.as_u64()), ProcessStatus::Crashed)
                            .await;
                        break;
                    }
                }
            }

            signal_channels.unregister(pid).await;
        });

        handle.abort_handle()
    }
}
