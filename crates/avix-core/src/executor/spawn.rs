use crate::types::{token::CapabilityToken, Pid};
use std::path::PathBuf;

pub struct SpawnParams {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub token: CapabilityToken,
    pub session_id: String,
    /// ATP connection session ID (from `ValidatedCmd.caller_session_id`).
    /// Used by `IpcExecutorFactory` for `event_bus.*` calls so the ownership gate
    /// (`conn.session_id == event.owner_session`) passes correctly.
    pub atp_session_id: String,
    /// System prompt from the agent manifest's `defaults.systemPrompt`.
    pub system_prompt: Option<String>,
    /// The resolved model name (from `--model` arg or `KernelConfig.models.default`).
    pub selected_model: String,
    /// Tools explicitly denied at spawn (not in the user's crew or capability set).
    #[allow(dead_code)]
    pub denied_tools: Vec<String>,
    /// Maximum context-window token limit for this agent (0 = unknown).
    pub context_limit: u64,
    /// Runtime directory for IPC sockets.
    pub runtime_dir: PathBuf,
    /// Invocation ID (UUID v4) generated at spawn by `ProcHandler`.
    /// Used by `RuntimeExecutor` to write conversation + finalize invocation record.
    pub invocation_id: String,
    /// When `Some(old_pid)`, the factory enters restore mode: loads the conversation
    /// JSONL from `<username>/.sessions/<session_id>/<old_pid>.jsonl`, injects it into
    /// the executor, then goes directly to `idle()` + `wait_for_next_goal()` without
    /// running an LLM turn. Set by crash-recovery restoration only.
    pub restore_from_pid: Option<u64>,
}
