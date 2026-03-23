use crate::types::{token::CapabilityToken, Pid};

pub struct SpawnParams {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub token: CapabilityToken,
    pub session_id: String,
    /// System prompt from the agent manifest's `defaults.systemPrompt`.
    pub system_prompt: Option<String>,
    /// The resolved model name (from `--model` arg or `KernelConfig.models.default`).
    pub selected_model: String,
}
