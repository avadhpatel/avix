use chrono::{DateTime, Utc};
use serde::Serialize;

use super::entry::{ProcessEntry, ProcessStatus, WaitingOn};

/// Serialisable snapshot of `/proc/<pid>/status.yaml`.
///
/// Constructed from a `ProcessEntry` (plus any live pipe records) and
/// written to the VFS by the kernel after every lifecycle event.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusFile {
    pub api_version: String,
    pub kind: String,
    pub metadata: AgentStatusMetadata,
    pub status: AgentStatusSpec,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusMetadata {
    pub name: String,
    pub pid: u64,
    pub spawned_at: DateTime<Utc>,
    pub spawned_by: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSpec {
    pub state: ProcessStatus,
    pub goal: String,
    pub context_used: u64,
    pub context_limit: u64,
    pub tool_calls_this_turn: u32,
    pub last_activity_at: DateTime<Utc>,
    pub waiting_on: Option<WaitingOn>,
    pub tools: AgentStatusTools,
    pub pipes: Vec<AgentStatusPipe>,
    pub signals: AgentStatusSignals,
    pub metrics: AgentStatusMetrics,
}

#[derive(Debug, Serialize)]
pub struct AgentStatusTools {
    pub granted: Vec<String>,
    pub denied: Vec<String>,
}

/// Snapshot of one pipe connection as it appears in `status.pipes`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusPipe {
    pub id: String,
    pub target_pid: u64,
    /// `"in"`, `"out"`, or `"bidirectional"`
    pub direction: String,
    /// `"open"`, `"closed"`, or `"draining"`
    pub state: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSignals {
    pub last_received: Option<String>,
    pub pending_count: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusMetrics {
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    /// Wall-clock seconds since the agent was spawned.
    pub wall_time_sec: u64,
}

impl AgentStatusFile {
    /// Build a status-file snapshot from a live `ProcessEntry`.
    ///
    /// `pipes` is the list of pipe connections currently open for this agent;
    /// pass `vec![]` when the caller cannot enumerate them.
    pub fn from_entry(entry: &ProcessEntry, pipes: Vec<AgentStatusPipe>) -> Self {
        let wall_time_sec = (Utc::now() - entry.spawned_at).num_seconds().max(0) as u64;

        AgentStatusFile {
            api_version: "avix/v1".into(),
            kind: "AgentStatus".into(),
            metadata: AgentStatusMetadata {
                name: entry.name.clone(),
                pid: entry.pid.as_u64(),
                spawned_at: entry.spawned_at,
                spawned_by: entry.spawned_by_user.clone(),
            },
            status: AgentStatusSpec {
                state: entry.status.clone(),
                goal: entry.goal.clone(),
                context_used: entry.context_used,
                context_limit: entry.context_limit,
                tool_calls_this_turn: entry.tool_chain_depth,
                last_activity_at: entry.last_activity_at,
                waiting_on: entry.waiting_on.clone(),
                tools: AgentStatusTools {
                    granted: entry.granted_tools.clone(),
                    denied: entry.denied_tools.clone(),
                },
                pipes,
                signals: AgentStatusSignals {
                    last_received: entry.last_signal_received.clone(),
                    pending_count: entry.pending_signal_count,
                },
                metrics: AgentStatusMetrics {
                    tokens_consumed: entry.tokens_consumed,
                    tool_calls_total: entry.tool_calls_total,
                    wall_time_sec,
                },
            },
        }
    }

    /// Serialise to YAML bytes.
    pub fn to_yaml(&self) -> Result<Vec<u8>, serde_yaml::Error> {
        serde_yaml::to_string(self).map(|s| s.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::entry::{ProcessKind, ProcessStatus};
    use crate::types::Pid;

    fn make_entry() -> ProcessEntry {
        ProcessEntry {
            pid: Pid::from_u64(42),
            name: "test-agent".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            spawned_by_user: "alice".into(),
            goal: "Research quantum computing".into(),
            granted_tools: vec!["fs/read".into(), "web/search".into()],
            denied_tools: vec!["send/email".into()],
            context_used: 5000,
            context_limit: 200_000,
            tool_chain_depth: 2,
            tokens_consumed: 14_200,
            tool_calls_total: 11,
            ..Default::default()
        }
    }

    #[test]
    fn process_status_serializes_all_six_variants() {
        let cases = [
            (ProcessStatus::Pending, "pending"),
            (ProcessStatus::Running, "running"),
            (ProcessStatus::Paused, "paused"),
            (ProcessStatus::Waiting, "waiting"),
            (ProcessStatus::Stopped, "stopped"),
            (ProcessStatus::Crashed, "crashed"),
        ];
        for (variant, expected) in cases {
            let yaml = serde_yaml::to_string(&variant).unwrap();
            assert_eq!(
                yaml.trim(),
                expected,
                "ProcessStatus::{expected} serialised incorrectly"
            );
        }
    }

    #[test]
    fn waiting_on_serializes_kebab_case() {
        let cases = [
            (WaitingOn::HumanApproval, "human-approval"),
            (WaitingOn::PipeRead, "pipe-read"),
            (WaitingOn::PipeWrite, "pipe-write"),
            (WaitingOn::Signal, "signal"),
        ];
        for (variant, expected) in cases {
            let yaml = serde_yaml::to_string(&variant).unwrap();
            assert_eq!(yaml.trim(), expected, "WaitingOn serialised incorrectly");
        }
    }

    #[test]
    fn agent_status_file_contains_required_yaml_keys() {
        let entry = make_entry();
        let file = AgentStatusFile::from_entry(&entry, vec![]);
        let yaml = String::from_utf8(file.to_yaml().unwrap()).unwrap();

        assert!(yaml.contains("kind: AgentStatus"), "missing kind");
        assert!(yaml.contains("state: running"), "missing state");
        assert!(yaml.contains("contextUsed: 5000"), "missing contextUsed");
        assert!(
            yaml.contains("contextLimit: 200000"),
            "missing contextLimit"
        );
        assert!(
            yaml.contains("tokensConsumed: 14200"),
            "missing tokensConsumed"
        );
        assert!(
            yaml.contains("toolCallsTotal: 11"),
            "missing toolCallsTotal"
        );
        assert!(yaml.contains("wallTimeSec:"), "missing wallTimeSec");
        assert!(yaml.contains("fs/read"), "missing granted tool");
        assert!(yaml.contains("send/email"), "missing denied tool");
    }

    #[test]
    fn wall_time_sec_is_non_negative() {
        let entry = make_entry();
        let file = AgentStatusFile::from_entry(&entry, vec![]);
        assert!(file.status.metrics.wall_time_sec < u64::MAX);
    }

    #[test]
    fn pipes_section_populated_from_argument() {
        let entry = make_entry();
        let pipes = vec![AgentStatusPipe {
            id: "pipe-001".into(),
            target_pid: 58,
            direction: "out".into(),
            state: "open".into(),
        }];
        let file = AgentStatusFile::from_entry(&entry, pipes);
        let yaml = String::from_utf8(file.to_yaml().unwrap()).unwrap();
        assert!(yaml.contains("pipe-001"), "missing pipe id");
        assert!(yaml.contains("targetPid: 58"), "missing targetPid");
    }
}
