use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::Pid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Agent,
    Service,
    Kernel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Paused,
    Waiting,
    Stopped,
    Crashed,
}

/// A live entry in the process table, analogous to `/proc/<pid>/status.yaml`.
///
/// The three capability-related fields are populated by `RuntimeExecutor` at
/// spawn time and kept in sync as the agent runs:
/// - `granted_tools` — the tool names currently in the agent's `CapabilityToken`
/// - `token_expires_at` — expiry timestamp for observability / `/proc` reads
/// - `tool_chain_depth` — number of tool calls dispatched in the current turn
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: Pid,
    pub name: String,
    pub kind: ProcessKind,
    pub status: ProcessStatus,
    pub parent: Option<Pid>,
    pub spawned_by_user: String,
    /// Tool names currently granted to this agent (from its CapabilityToken).
    #[serde(default)]
    pub granted_tools: Vec<String>,
    /// When the agent's current CapabilityToken expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_expires_at: Option<DateTime<Utc>>,
    /// Number of tool calls dispatched in the current turn (resets each turn).
    #[serde(default)]
    pub tool_chain_depth: u32,
}
