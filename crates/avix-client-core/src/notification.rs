use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::atp::types::{HilOutcome, HilRequestBody};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationKind {
    Hil,
    AgentExit,
    SysAlert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub agent_pid: Option<u64>,
    pub session_id: Option<String>,
    pub message: String,
    pub hil: Option<HilState>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HilState {
    pub hil_id: String,
    pub pid: u64,
    pub approval_token: String,
    pub prompt: String,
    pub timeout_secs: u32,
    pub outcome: Option<HilOutcome>,
}

impl Notification {
    pub fn from_hil_request(body: &HilRequestBody) -> Self {
        Self {
            id: body.hil_id.clone(),
            kind: NotificationKind::Hil,
            agent_pid: Some(body.pid),
            session_id: Some(body.session_id.clone()),
            message: body.prompt.clone(),
            hil: Some(HilState {
                hil_id: body.hil_id.clone(),
                pid: body.pid,
                approval_token: body.approval_token.clone(),
                prompt: body.prompt.clone(),
                timeout_secs: body.timeout_secs,
                outcome: None,
            }),
            created_at: Utc::now(),
            resolved_at: None,
            read: false,
        }
    }

    pub fn from_agent_exit(pid: u64, session_id: &str, reason: Option<&str>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::AgentExit,
            agent_pid: Some(pid),
            session_id: Some(session_id.to_string()),
            message: reason.map_or_else(
                || "Agent exited".to_string(),
                |r| format!("Agent exited: {r}"),
            ),
            hil: None,
            created_at: Utc::now(),
            resolved_at: None,
            read: false,
        }
    }

    pub fn from_sys_alert(_level: &str, message: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind: NotificationKind::SysAlert,
            agent_pid: None,
            session_id: None,
            message: message.to_string(),
            hil: None,
            created_at: Utc::now(),
            resolved_at: None,
            read: false,
        }
    }
}

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

use crate::trace::ClientTracer;

impl std::fmt::Debug for NotificationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationStore").finish_non_exhaustive()
    }
}

pub struct NotificationStore {
    inner: Arc<Mutex<Vec<Notification>>>,
    changed: broadcast::Sender<()>,
    tracer: Arc<ClientTracer>,
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(vec![])),
            changed: broadcast::channel(1).0,
            tracer: ClientTracer::noop(),
        }
    }

    /// Attach a `ClientTracer` so notification events are written to trace files.
    pub fn with_tracer(mut self, tracer: Arc<ClientTracer>) -> Self {
        self.tracer = tracer;
        self
    }

    pub async fn add(&self, n: Notification) {
        let kind_str = format!("{:?}", n.kind);
        let id = n.id.clone();
        let message = n.message.clone();
        let agent_pid = n.agent_pid;
        // Trace HIL requests specifically.
        if let (Some(hil), true) = (&n.hil, n.kind == NotificationKind::Hil) {
            self.tracer.hil_request(hil.pid, &hil.hil_id, &hil.prompt);
        }
        self.tracer
            .notification_added(&kind_str, &id, &message, agent_pid);

        let mut inner = self.inner.lock().await;
        inner.push(n.clone());
        let count = inner.iter().filter(|n| !n.read).count();
        inner.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        drop(inner);
        let _ = self.changed.send(());
        debug!("Notif add {:?} unread={}", message, count);
    }

    pub async fn resolve_hil(&self, hil_id: &str, outcome: HilOutcome) {
        let outcome_str = format!("{:?}", outcome);
        self.tracer.hil_resolved(hil_id, &outcome_str);

        let mut inner = self.inner.lock().await;
        if let Some(notif) = inner
            .iter_mut()
            .find(|n| n.id == hil_id && n.kind == NotificationKind::Hil)
        {
            if notif.hil.as_ref().unwrap().outcome.is_none() {
                notif.hil.as_mut().unwrap().outcome = Some(outcome);
                notif.resolved_at = Some(Utc::now());
            }
        }
        drop(inner);
        let _ = self.changed.send(());
    }

    pub async fn mark_read(&self, id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(notif) = inner.iter_mut().find(|n| n.id == id) {
            notif.read = true;
        }
        drop(inner);
        let _ = self.changed.send(());
    }

    pub async fn all(&self) -> Vec<Notification> {
        let inner = self.inner.lock().await;
        inner.clone()
    }

    pub async fn unread_count(&self) -> usize {
        let inner = self.inner.lock().await;
        inner.iter().filter(|n| !n.read).count()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changed.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_increases_unread_count() {
        let store = NotificationStore::new();
        assert_eq!(store.unread_count().await, 0);
        store
            .add(Notification::from_sys_alert("info", "test"))
            .await;
        assert_eq!(store.unread_count().await, 1);
    }

    #[tokio::test]
    async fn mark_read_decreases_unread_count() {
        let store = NotificationStore::new();
        store
            .add(Notification::from_sys_alert("info", "test"))
            .await;
        assert_eq!(store.unread_count().await, 1);
        let all = store.all().await;
        let id = all[0].id.clone();
        store.mark_read(&id).await;
        assert_eq!(store.unread_count().await, 0);
    }

    #[tokio::test]
    async fn resolve_hil_sets_outcome() {
        let store = NotificationStore::new();
        let hil_req = HilRequestBody {
            hil_id: "test-hil".to_string(),
            pid: 123,
            session_id: "sess".to_string(),
            approval_token: "token".to_string(),
            hil_type: "capability_upgrade".to_string(),
            tool: Some("fs/write".to_string()),
            reason: Some("needs write access".to_string()),
            prompt: "approve?".to_string(),
            timeout_secs: 30,
            urgency: "normal".to_string(),
        };
        let notif = Notification::from_hil_request(&hil_req);
        store.add(notif).await;
        store.resolve_hil("test-hil", HilOutcome::Approved).await;
        let all = store.all().await;
        assert_eq!(
            all[0].hil.as_ref().unwrap().outcome,
            Some(HilOutcome::Approved)
        );
    }

    #[tokio::test]
    async fn changed_signal_fires_on_add() {
        let store = NotificationStore::new();
        let mut rx = store.subscribe();
        store
            .add(Notification::from_sys_alert("info", "test"))
            .await;
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn all_returns_newest_first() {
        let store = NotificationStore::new();
        let mut n1 = Notification::from_sys_alert("info", "first");
        n1.created_at = chrono::DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut n2 = Notification::from_sys_alert("info", "second");
        n2.created_at = chrono::DateTime::parse_from_rfc3339("2023-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        store.add(n1).await;
        store.add(n2).await;
        let all = store.all().await;
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].message, "second");
        assert_eq!(all[1].message, "first");
    }
}
