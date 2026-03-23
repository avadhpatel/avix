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

/// Six-state lifecycle matching the AgentStatus spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    /// Spawned but not yet started; waiting for kernel resource allocation.
    Pending,
    /// Actively executing goal.
    Running,
    /// Suspended by `SIGPAUSE`; consuming no resources.
    Paused,
    /// Blocked on an external event (see `waiting_on`).
    Waiting,
    /// Gracefully shut down via `SIGSTOP`.
    Stopped,
    /// Terminated unexpectedly; kernel may restore from snapshot.
    Crashed,
}

/// What an agent in the `Waiting` state is blocked on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitingOn {
    HumanApproval,
    PipeRead,
    PipeWrite,
    Signal,
}

/// A live entry in the process table, analogous to `/proc/<pid>/status.yaml`.
///
/// The capability-related fields are populated by `RuntimeExecutor` at spawn
/// and kept in sync as the agent runs:
/// - `granted_tools` — tool names currently in the agent's `CapabilityToken`
/// - `token_expires_at` — expiry timestamp for observability / `/proc` reads
/// - `tool_chain_depth` — tool calls dispatched in the current turn
///
/// The metrics fields (`tokens_consumed`, `tool_calls_total`, `context_used`)
/// and lifecycle fields (`last_activity_at`, `waiting_on`, `last_signal_received`)
/// are updated via `ProcessTable` helper methods.
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

    // ── extended fields ───────────────────────────────────────────────────────
    /// Goal string passed to the agent at spawn.
    #[serde(default)]
    pub goal: String,
    /// When this agent was spawned.
    pub spawned_at: DateTime<Utc>,
    /// Tokens currently occupying the working context window.
    #[serde(default)]
    pub context_used: u64,
    /// Maximum context-window token limit for this agent.
    #[serde(default)]
    pub context_limit: u64,
    /// Timestamp of the last agent activity (tool call or LLM turn start).
    pub last_activity_at: DateTime<Utc>,
    /// What the agent is waiting on when `status == Waiting`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<WaitingOn>,
    /// Tools explicitly denied at spawn (e.g. not in the user's crew).
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// Name of the last signal received by this agent (e.g. `"SIGPAUSE"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_signal_received: Option<String>,
    /// Number of unhandled pending signals in the delivery queue.
    #[serde(default)]
    pub pending_signal_count: u32,
    /// Total tokens consumed by this agent in this session.
    #[serde(default)]
    pub tokens_consumed: u64,
    /// Total tool calls dispatched over the agent's lifetime.
    #[serde(default)]
    pub tool_calls_total: u32,
}

impl Default for ProcessEntry {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            pid: Pid::new(0),
            name: String::new(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Pending,
            parent: None,
            spawned_by_user: String::new(),
            granted_tools: Vec::new(),
            token_expires_at: None,
            tool_chain_depth: 0,
            goal: String::new(),
            spawned_at: now,
            context_used: 0,
            context_limit: 0,
            last_activity_at: now,
            waiting_on: None,
            denied_tools: Vec::new(),
            last_signal_received: None,
            pending_signal_count: 0,
            tokens_consumed: 0,
            tool_calls_total: 0,
        }
    }
}
