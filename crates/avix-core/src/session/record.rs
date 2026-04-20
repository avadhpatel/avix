use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use tracing::instrument;

// ── PidInvocationMeta ─────────────────────────────────────────────────────────

/// Per-PID metadata stored on the session for every invocation that ran within it.
/// Recorded at spawn time so the session record is self-describing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PidInvocationMeta {
    pub pid: u64,
    pub invocation_id: String,
    pub agent_name: String,
    #[serde(default)]
    pub agent_version: String,
    pub spawned_at: DateTime<Utc>,
}

// ── SessionStatus ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    #[default]
    Running,
    Idle,
    /// Non-terminal — all invocations in this session are paused. Can be resumed.
    Paused,
    Completed,
    Failed,
    Archived,
}

// ── SessionRecord ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub title: String,
    pub goal: String,
    pub username: String,
    pub spawned_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub status: SessionStatus,
    pub summary: Option<String>,
    pub tokens_total: u64,
    pub origin_agent: String,
    pub primary_agent: String,
    pub participants: Vec<String>,
    /// PID that created this session. Always set to a valid (non-zero) PID at creation.
    pub owner_pid: u64,
    /// All currently active PIDs contributing to this session.
    #[serde(default)]
    pub pids: Vec<u64>,
    /// Metadata for every invocation (PID) that has ever run in this session.
    #[serde(default)]
    pub invocation_pids: Vec<PidInvocationMeta>,
}

impl SessionRecord {
    #[instrument]
    pub fn new(
        id: Uuid,
        username: String,
        origin_agent: String,
        title: String,
        goal: String,
        owner_pid: u64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            parent_id: None,
            project_id: None,
            title,
            goal,
            username,
            spawned_at: now,
            last_updated: now,
            status: SessionStatus::Running,
            summary: None,
            tokens_total: 0,
            origin_agent: origin_agent.clone(),
            primary_agent: origin_agent,
            participants: vec![],
            owner_pid,
            pids: vec![owner_pid],
            invocation_pids: vec![],
        }
    }

    #[instrument]
    pub fn mark_idle(&mut self) {
        self.status = SessionStatus::Idle;
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn mark_running(&mut self) {
        self.status = SessionStatus::Running;
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn mark_completed(&mut self) {
        self.status = SessionStatus::Completed;
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn mark_failed(&mut self) {
        self.status = SessionStatus::Failed;
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn mark_paused(&mut self) {
        self.status = SessionStatus::Paused;
        self.last_updated = Utc::now();
    }

    /// Record per-PID invocation metadata on the session (dedup by pid).
    #[instrument]
    pub fn add_invocation_pid(&mut self, meta: PidInvocationMeta) {
        if !self.invocation_pids.iter().any(|m| m.pid == meta.pid) {
            tracing::debug!(pid = meta.pid, invocation_id = %meta.invocation_id, "adding invocation pid meta to session");
            self.invocation_pids.push(meta);
        }
        self.last_updated = Utc::now();
    }

    /// Register an additional PID as active in this session (e.g. a child agent).
    /// `owner_pid` is immutable after construction.
    #[instrument]
    pub fn add_pid(&mut self, pid: u64) {
        if !self.pids.contains(&pid) {
            self.pids.push(pid);
        }
        self.last_updated = Utc::now();
    }

    /// Remove a PID from the active set (called on agent exit).
    #[instrument]
    pub fn remove_pid(&mut self, pid: u64) {
        self.pids.retain(|&p| p != pid);
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn add_participant(&mut self, agent_name: &str, make_primary: bool) {
        if !self.participants.contains(&agent_name.to_string()) {
            self.participants.push(agent_name.to_string());
        }
        if make_primary {
            self.primary_agent = agent_name.to_string();
        }
        self.last_updated = Utc::now();
    }

    #[instrument]
    pub fn set_primary(&mut self, agent_name: &str) {
        let agent_str = agent_name.to_string();
        if self.participants.contains(&agent_str) || agent_name == self.origin_agent.as_str() {
            self.primary_agent = agent_str;
            self.last_updated = Utc::now();
        }
    }

    #[instrument]
    pub fn add_tokens(&mut self, tokens: u64) {
        self.tokens_total += tokens;
        self.last_updated = Utc::now();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record() -> SessionRecord {
        SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "researcher".to_string(),
            "Research Q4".to_string(),
            "Analyze market trends".to_string(),
            42,
        )
    }

    #[test]
    fn session_record_new_defaults() {
        let r = make_record();
        assert_eq!(r.status, SessionStatus::Running);
        assert!(r.participants.is_empty());
        assert_eq!(r.origin_agent, "researcher");
        assert_eq!(r.primary_agent, "researcher");
        assert_eq!(r.owner_pid, 42);
        assert_eq!(r.pids, vec![42]);
    }

    #[test]
    fn mark_idle_sets_status_and_timestamp() {
        let mut r = make_record();
        let before = r.last_updated;
        std::thread::sleep(std::time::Duration::from_millis(2));
        r.mark_idle();
        assert_eq!(r.status, SessionStatus::Idle);
        assert!(r.last_updated > before);
    }

    #[test]
    fn mark_running_sets_status_and_timestamp() {
        let mut r = make_record();
        r.mark_idle();
        let before = r.last_updated;
        std::thread::sleep(std::time::Duration::from_millis(2));
        r.mark_running();
        assert_eq!(r.status, SessionStatus::Running);
        assert!(r.last_updated > before);
    }

    #[test]
    fn add_participant_with_make_primary() {
        let mut r = make_record();
        r.add_participant("coder", true);
        assert!(r.participants.contains(&"coder".to_string()));
        assert_eq!(r.primary_agent, "coder");
    }

    #[test]
    fn add_participant_avoids_duplicates() {
        let mut r = make_record();
        r.add_participant("coder", false);
        r.add_participant("coder", false);
        assert_eq!(r.participants.len(), 1);
    }

    #[test]
    fn set_primary_swaps_primary_agent() {
        let mut r = make_record();
        r.add_participant("coder", true);
        r.set_primary("researcher");
        assert_eq!(r.primary_agent, "researcher");
    }

    #[test]
    fn set_primary_with_origin_promotes_back() {
        let mut r = make_record();
        r.add_participant("coder", true);
        r.set_primary("researcher");
        assert_eq!(r.primary_agent, "researcher");
    }

    #[test]
    fn session_status_serialises_lowercase() {
        for (status, expected) in [
            (SessionStatus::Running, "running"),
            (SessionStatus::Idle, "idle"),
            (SessionStatus::Paused, "paused"),
            (SessionStatus::Completed, "completed"),
            (SessionStatus::Failed, "failed"),
            (SessionStatus::Archived, "archived"),
        ] {
            let ser = serde_json::to_string(&status).unwrap();
            let ser = ser.trim();
            assert_eq!(ser, format!("\"{expected}\""));
        }
    }

    #[test]
    fn roundtrip_json() {
        let r = make_record();
        let json = serde_json::to_string(&r).unwrap();
        let r2: SessionRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(r2.id, r.id);
        assert_eq!(r2.username, r.username);
        assert_eq!(r2.origin_agent, r.origin_agent);
        assert_eq!(r2.status, r.status);
    }

    #[test]
    fn new_initialises_owner_pid_and_pids() {
        let r = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "agent".to_string(),
            "title".to_string(),
            "goal".to_string(),
            7,
        );
        assert_eq!(r.owner_pid, 7);
        assert_eq!(r.pids, vec![7]);
    }

    #[test]
    fn add_pid_appends_child_pid_without_changing_owner() {
        let mut r = make_record(); // owner_pid = 42
        r.add_pid(99);
        assert_eq!(r.owner_pid, 42);
        assert_eq!(r.pids, vec![42, 99]);
    }

    #[test]
    fn add_pid_avoids_duplicates() {
        let mut r = make_record();
        r.add_pid(42); // already in pids from new()
        assert_eq!(r.pids.len(), 1);
    }

    #[test]
    fn remove_pid_removes_correctly() {
        let mut r = make_record(); // owner_pid = 42, pids = [42]
        r.add_pid(99);
        r.remove_pid(99);
        assert_eq!(r.pids, vec![42]);
        assert_eq!(r.owner_pid, 42); // owner_pid is immutable
    }

    #[test]
    fn mark_paused_sets_status() {
        let mut r = make_record();
        r.mark_paused();
        assert_eq!(r.status, SessionStatus::Paused);
    }

    #[test]
    fn owner_pid_required_in_serialised_form() {
        // ownerPid must be present in the JSON output.
        let r = make_record();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"ownerPid\":42"));
        assert!(json.contains("\"pids\":[42]"));
    }
}
