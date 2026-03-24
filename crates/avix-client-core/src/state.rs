use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ClientConfig;
use crate::atp::dispatcher::Dispatcher;
use crate::atp::event_emitter::EventEmitter;
use crate::notification::NotificationStore;
use crate::atp::types::AgentStatus;

/// Shared application state. Both clients wrap this in Arc<RwLock<AppState>>.
#[derive(Debug)]
pub struct AppState {
    pub config: ClientConfig,
    pub dispatcher: Option<Arc<Dispatcher>>,
    pub emitter: Option<EventEmitter>,
    pub notifications: Arc<NotificationStore>,
    pub agents: Arc<RwLock<Vec<ActiveAgent>>>,
    pub connection_status: ConnectionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected { session_id: String },
    Reconnecting { attempt: u32 },
}

#[derive(Debug, Clone)]
pub struct ActiveAgent {
    pub pid: u64,
    pub name: String,
    pub session_id: String,
    pub status: AgentStatus,
    pub goal: String,
}

impl AppState {
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            dispatcher: None,
            emitter: None,
            notifications: Arc::new(NotificationStore::new()),
            agents: Arc::new(RwLock::new(vec![])),
            connection_status: ConnectionStatus::Disconnected,
        }
    }
}

/// Type alias used everywhere in both clients.
pub type SharedState = Arc<RwLock<AppState>>;

pub fn new_shared(config: ClientConfig) -> SharedState {
    Arc::new(RwLock::new(AppState::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_shared_starts_disconnected() {
        let cfg = ClientConfig::default();
        let state = new_shared(cfg);
        let s = state.try_read().unwrap();
        assert_eq!(s.connection_status, ConnectionStatus::Disconnected);
        assert!(s.dispatcher.is_none());
    }
}