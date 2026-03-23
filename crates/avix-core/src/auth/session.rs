use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::types::{Pid, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Active,
    /// Disconnected; reconnect grace window in progress.
    Idle,
    Closed,
}

/// A live authenticated session tracked by `AuthService`.
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    /// Username — kept for backward-compatibility with callers.
    pub identity_name: String,
    pub uid: u32,
    pub role: Role,
    pub crews: Vec<String>,
    pub scope: Vec<String>,
    pub state: SessionState,
    pub connected_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    /// Set when the connection drops; cleared on reconnect.
    pub idle_since: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_reason: Option<String>,
    /// PIDs of agents spawned in this session.
    pub agents: Vec<Pid>,
    /// TTL for expiry checks (mirrors `AuthService::ttl`).
    pub ttl: Duration,
}

impl SessionEntry {
    pub fn is_expired(&self) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.connected_at)
            .to_std()
            .unwrap_or(Duration::ZERO);
        elapsed > self.ttl
    }

    /// Transition to idle and record the disconnect time.
    pub fn mark_idle(&mut self) {
        self.state = SessionState::Idle;
        self.idle_since = Some(Utc::now());
    }

    /// Transition back to active (reconnect within grace window).
    pub fn mark_active(&mut self) {
        self.state = SessionState::Active;
        self.idle_since = None;
        self.last_activity_at = Utc::now();
    }

    /// Transition to closed with a reason string.
    pub fn mark_closed(&mut self, reason: impl Into<String>) {
        self.state = SessionState::Closed;
        self.closed_at = Some(Utc::now());
        self.closed_reason = Some(reason.into());
    }

    /// True when the 60-second reconnect grace window has expired.
    pub fn grace_expired(&self) -> bool {
        match self.idle_since {
            Some(t) => Utc::now().signed_duration_since(t) > chrono::Duration::seconds(60),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry() -> SessionEntry {
        SessionEntry {
            session_id: "s-001".into(),
            identity_name: "alice".into(),
            uid: 1001,
            role: Role::User,
            crews: vec![],
            scope: vec!["proc".into()],
            state: SessionState::Active,
            connected_at: Utc::now(),
            last_activity_at: Utc::now(),
            idle_since: None,
            closed_at: None,
            closed_reason: None,
            agents: vec![],
            ttl: Duration::from_secs(3600),
        }
    }

    #[test]
    fn mark_idle_sets_state_and_timestamp() {
        let mut e = make_entry();
        e.mark_idle();
        assert_eq!(e.state, SessionState::Idle);
        assert!(e.idle_since.is_some());
    }

    #[test]
    fn mark_active_clears_idle() {
        let mut e = make_entry();
        e.mark_idle();
        e.mark_active();
        assert_eq!(e.state, SessionState::Active);
        assert!(e.idle_since.is_none());
    }

    #[test]
    fn grace_not_expired_immediately_after_idle() {
        let mut e = make_entry();
        e.mark_idle();
        assert!(!e.grace_expired());
    }

    #[test]
    fn mark_closed_sets_reason_and_timestamp() {
        let mut e = make_entry();
        e.mark_closed("ping timeout");
        assert_eq!(e.state, SessionState::Closed);
        assert_eq!(e.closed_reason.as_deref(), Some("ping timeout"));
        assert!(e.closed_at.is_some());
    }

    #[test]
    fn is_expired_returns_false_for_fresh_entry() {
        let e = make_entry();
        assert!(!e.is_expired());
    }
}
