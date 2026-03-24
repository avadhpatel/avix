use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilState {
    pub hil_id: String,
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
            message: reason.map_or_else(|| "Agent exited".to_string(), |r| format!("Agent exited: {r}")),
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
use tokio::sync::{Mutex, broadcast};

impl std::fmt::Debug for NotificationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationStore").finish_non_exhaustive()
    }
}

pub struct NotificationStore {
    inner: Arc<Mutex<Vec<Notification>>>,
    changed: broadcast::Sender<()>,
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
        }
    }

    pub async fn add(&self, n: Notification) {
        let mut inner = self.inner.lock().await;
        inner.push(n);
        inner.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        drop(inner);
        let _ = self.changed.send(());
    }

    pub async fn resolve_hil(&self, hil_id: &str, outcome: HilOutcome) {
        let mut inner = self.inner.lock().await;
        if let Some(notif) = inner.iter_mut().find(|n| n.id == hil_id && n.kind == NotificationKind::Hil) {
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