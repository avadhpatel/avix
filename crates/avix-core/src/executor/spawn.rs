use crate::types::{token::CapabilityToken, Pid};

pub struct SpawnParams {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub token: CapabilityToken,
    pub session_id: String,
}
