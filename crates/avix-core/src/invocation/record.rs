use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── InvocationStatus ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvocationStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Killed,
}

// ── InvocationRecord ──────────────────────────────────────────────────────────

/// Persistent record of a single agent execution (one spawn → exit cycle).
///
/// Survives daemon restart; stored in redb and mirrored as a YAML artefact at
/// `/users/<username>/agents/<agent_name>/invocations/<id>.yaml`.
///
/// Links: docs/architecture/06-agents.md
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationRecord {
    /// UUID v4 — permanent identifier, independent of the recycled `pid`.
    pub id: String,
    pub agent_name: String,
    pub username: String,
    /// Runtime PID at the time of spawn (informational only; not stable).
    pub pid: u32,
    pub goal: String,
    pub session_id: String,
    pub spawned_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub status: InvocationStatus,
    #[serde(default)]
    pub tokens_consumed: u64,
    #[serde(default)]
    pub tool_calls_total: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_reason: Option<String>,
}

impl InvocationRecord {
    pub fn new(
        id: String,
        agent_name: String,
        username: String,
        pid: u32,
        goal: String,
        session_id: String,
    ) -> Self {
        Self {
            id,
            agent_name,
            username,
            pid,
            goal,
            session_id,
            spawned_at: Utc::now(),
            ended_at: None,
            status: InvocationStatus::Running,
            tokens_consumed: 0,
            tool_calls_total: 0,
            exit_reason: None,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record() -> InvocationRecord {
        InvocationRecord::new(
            "inv-001".into(),
            "researcher".into(),
            "alice".into(),
            42,
            "Analyse Q3 report".into(),
            "sess-abc".into(),
        )
    }

    #[test]
    fn default_status_is_running() {
        let r = make_record();
        assert_eq!(r.status, InvocationStatus::Running);
    }

    #[test]
    fn roundtrip_json() {
        let r = make_record();
        let json = serde_json::to_string(&r).unwrap();
        let r2: InvocationRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.id, r.id);
        assert_eq!(r2.agent_name, r.agent_name);
        assert_eq!(r2.username, r.username);
        assert_eq!(r2.status, r.status);
    }

    #[test]
    fn exit_reason_skipped_when_none() {
        let r = make_record();
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("exitReason"));
    }
}
