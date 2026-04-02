use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── SessionStatus ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    #[default]
    Running,
    Idle,
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
}

impl SessionRecord {
    pub fn new(
        id: Uuid,
        username: String,
        origin_agent: String,
        title: String,
        goal: String,
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
        }
    }

    pub fn mark_idle(&mut self) {
        self.status = SessionStatus::Idle;
        self.last_updated = Utc::now();
    }

    pub fn mark_running(&mut self) {
        self.status = SessionStatus::Running;
        self.last_updated = Utc::now();
    }

    pub fn mark_completed(&mut self) {
        self.status = SessionStatus::Completed;
        self.last_updated = Utc::now();
    }

    pub fn mark_failed(&mut self) {
        self.status = SessionStatus::Failed;
        self.last_updated = Utc::now();
    }

    pub fn add_participant(&mut self, agent_name: &str, make_primary: bool) {
        if !self.participants.contains(&agent_name.to_string()) {
            self.participants.push(agent_name.to_string());
        }
        if make_primary {
            self.primary_agent = agent_name.to_string();
        }
        self.last_updated = Utc::now();
    }

    pub fn set_primary(&mut self, agent_name: &str) {
        let agent_str = agent_name.to_string();
        if self.participants.contains(&agent_str) || agent_name == self.origin_agent.as_str() {
            self.primary_agent = agent_str;
            self.last_updated = Utc::now();
        }
    }

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
        )
    }

    #[test]
    fn session_record_new_defaults() {
        let r = make_record();
        assert_eq!(r.status, SessionStatus::Running);
        assert!(r.participants.is_empty());
        assert_eq!(r.origin_agent, "researcher");
        assert_eq!(r.primary_agent, "researcher");
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
}
