use super::spawn::SpawnParams;

/// Decouples `ProcHandler` from the concrete `RuntimeExecutor` implementation.
///
/// The kernel proc layer calls `launch()` when an agent is spawned.  The factory
/// is responsible for building the LLM client, tool registry, and any other
/// executor dependencies, then running the executor as a background tokio task.
///
/// Returning an `AbortHandle` lets the kernel forcibly stop the task when a
/// `SIGKILL` is delivered (ADR-05: fresh connection per call; no persistent state
/// survives across task abort).
pub trait AgentExecutorFactory: Send + Sync {
    /// Spawn the executor as a background tokio task and return an abort handle.
    fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle;
}
