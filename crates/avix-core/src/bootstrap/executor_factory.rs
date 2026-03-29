use std::sync::Arc;

use tracing::{info, warn};

use crate::executor::factory::AgentExecutorFactory;
use crate::executor::runtime_executor::MockToolRegistry;
use crate::executor::runtime_executor::RuntimeExecutor;
use crate::executor::spawn::SpawnParams;
use crate::llm_client::IpcLlmClient;
use crate::process::entry::ProcessStatus;
use crate::process::table::ProcessTable;
use crate::types::Pid;

/// Concrete `AgentExecutorFactory` wired into the kernel bootstrap.
///
/// For each `launch()` call it:
///   1. Resolves the `llm.svc` socket path from `AVIX_LLM_SOCK` env var or
///      `params.runtime_dir/llm.sock` (derived at launch time, not stored at construction).
///   2. Creates an `IpcLlmClient` pointed at that socket.
///   3. Builds a `RuntimeExecutor` via `spawn_with_registry`.
///   4. Runs `run_with_client` inside a detached tokio task.
///   5. Updates the process table status to `Stopped` (success) or `Crashed` (error).
///   6. Returns the task's `AbortHandle` so `kernel/proc/kill` can stop it.
pub struct IpcExecutorFactory {
    /// Shared process table — used to update agent status on exit.
    process_table: Arc<ProcessTable>,
}

impl IpcExecutorFactory {
    pub fn new(process_table: Arc<ProcessTable>) -> Self {
        Self { process_table }
    }
}

impl AgentExecutorFactory for IpcExecutorFactory {
    fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle {
        // Derive the llm.svc socket from the agent's runtime_dir.  By the time
        // an agent is launched phase-3 will have started llm.svc at this path.
        let llm_sock = params.runtime_dir.join("llm.sock");
        let process_table = Arc::clone(&self.process_table);

        let pid = params.pid;
        let goal = params.goal.clone();
        let session_id = params.session_id.clone();

        let handle = tokio::spawn(async move {
            let registry = Arc::new(MockToolRegistry::new());
            let llm_client = IpcLlmClient::new(
                llm_sock.to_string_lossy().to_string(),
                pid.as_u32(),
                session_id,
            );

            let mut executor = match RuntimeExecutor::spawn_with_registry(params, registry).await {
                Ok(e) => e,
                Err(err) => {
                    warn!(pid = pid.as_u32(), error = %err, "executor spawn failed");
                    let _ = process_table.set_status(pid, ProcessStatus::Crashed).await;
                    return;
                }
            };

            info!(pid = pid.as_u32(), "executor started");

            match executor.run_with_client(&goal, &llm_client).await {
                Ok(_) => {
                    info!(pid = pid.as_u32(), "executor finished");
                    let _ = process_table.set_status(pid, ProcessStatus::Stopped).await;
                }
                Err(err) => {
                    warn!(pid = pid.as_u32(), error = %err, "executor crashed");
                    let _ = process_table
                        .set_status(Pid::new(pid.as_u32()), ProcessStatus::Crashed)
                        .await;
                }
            }
        });

        handle.abort_handle()
    }
}
