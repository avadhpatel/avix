use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── AgentRole ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    #[default]
    Primary,
    Subordinate,
}

// ── AgentRef ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub pid: u32,
    pub name: String,
    pub role: AgentRole,
}

// ── QuotaSnapshot ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    #[serde(default)]
    pub tokens_used: u64,
    pub tokens_limit: u64,
    #[serde(default)]
    pub agents_running: u32,
    pub agents_limit: u32,
}

// ── SessionState ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Active,
    Idle,
    Closed,
}

// ── SessionEntry ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub session_id: String,
    pub username: String,
    #[serde(default)]
    pub uid: u32,

    // spec fields (written to VFS manifest)
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default = "default_tty")]
    pub tty: bool,
    pub working_directory: String,
    #[serde(default)]
    pub agents: Vec<AgentRef>,
    #[serde(default)]
    pub quota_snapshot: QuotaSnapshot,

    // status fields (written to VFS manifest)
    #[serde(default)]
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_reason: Option<String>,

    // internal — stored in redb, NOT written to VFS manifest
    #[serde(default)]
    pub messages: Vec<serde_json::Value>,
    #[serde(default)]
    pub agent_name: String,
    #[serde(default)]
    pub goal: String,
}

fn default_shell() -> String {
    "/bin/sh".into()
}
fn default_tty() -> bool {
    true
}

impl SessionEntry {
    /// Create a new session with sensible defaults.
    pub fn new(
        session_id: String,
        username: String,
        uid: u32,
        quota_snapshot: QuotaSnapshot,
    ) -> Self {
        let now = Utc::now();
        let working_directory = format!("/users/{}/workspace", username);
        Self {
            session_id,
            username: username.clone(),
            uid,
            shell: default_shell(),
            tty: default_tty(),
            working_directory,
            agents: vec![],
            quota_snapshot,
            state: SessionState::Active,
            created_at: now,
            last_activity_at: now,
            closed_at: None,
            closed_reason: None,
            messages: vec![],
            agent_name: String::new(),
            goal: String::new(),
        }
    }

    /// Attach an agent to this session.
    pub fn add_agent(&mut self, pid: u32, name: String, role: AgentRole) {
        self.agents.push(AgentRef { pid, name, role });
        self.last_activity_at = Utc::now();
    }

    /// Close the session with a reason.
    pub fn close(&mut self, reason: impl Into<String>) {
        let now = Utc::now();
        self.state = SessionState::Closed;
        self.closed_at = Some(now);
        self.closed_reason = Some(reason.into());
        self.last_activity_at = now;
    }

    /// Mark session as idle.
    pub fn mark_idle(&mut self) {
        self.state = SessionState::Idle;
        self.last_activity_at = Utc::now();
    }

    /// Return to active from idle.
    pub fn mark_active(&mut self) {
        self.state = SessionState::Active;
        self.last_activity_at = Utc::now();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session() -> SessionEntry {
        SessionEntry::new(
            "sess-001".into(),
            "alice".into(),
            1001,
            QuotaSnapshot {
                tokens_limit: 500_000,
                agents_limit: 5,
                ..Default::default()
            },
        )
    }

    // T-SMA-01
    #[test]
    fn session_entry_new_defaults() {
        let entry = make_session();
        assert_eq!(entry.shell, "/bin/sh");
        assert!(entry.tty);
        assert_eq!(entry.working_directory, "/users/alice/workspace");
        assert!(entry.agents.is_empty());
        assert_eq!(entry.state, SessionState::Active);
        assert!(entry.closed_at.is_none());
        assert!(entry.closed_reason.is_none());
    }

    // T-SMA-02
    #[test]
    fn add_agent_appends_agent_ref() {
        let mut entry = make_session();
        let before = entry.last_activity_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        entry.add_agent(57, "researcher".into(), AgentRole::Primary);
        assert_eq!(entry.agents.len(), 1);
        assert_eq!(entry.agents[0].pid, 57);
        assert_eq!(entry.agents[0].role, AgentRole::Primary);
        assert!(entry.last_activity_at >= before);
    }

    // T-SMA-03
    #[test]
    fn close_sets_terminal_state() {
        let mut entry = make_session();
        entry.close("user logged out");
        assert_eq!(entry.state, SessionState::Closed);
        assert!(entry.closed_at.is_some());
        assert_eq!(entry.closed_reason.as_deref(), Some("user logged out"));
    }

    // T-SMA-04
    #[test]
    fn idle_active_cycle() {
        let mut entry = make_session();
        entry.mark_idle();
        assert_eq!(entry.state, SessionState::Idle);
        entry.mark_active();
        assert_eq!(entry.state, SessionState::Active);
    }

    // T-SMA-09
    #[test]
    fn session_state_serialises_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&SessionState::Active).unwrap().trim(),
            "active"
        );
        assert_eq!(
            serde_yaml::to_string(&SessionState::Idle).unwrap().trim(),
            "idle"
        );
        assert_eq!(
            serde_yaml::to_string(&SessionState::Closed).unwrap().trim(),
            "closed"
        );
    }

    // T-SMA-10
    #[test]
    fn agent_role_serialises_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&AgentRole::Primary).unwrap().trim(),
            "primary"
        );
        assert_eq!(
            serde_yaml::to_string(&AgentRole::Subordinate)
                .unwrap()
                .trim(),
            "subordinate"
        );
    }
}
