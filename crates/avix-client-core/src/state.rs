use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::atp::dispatcher::Dispatcher;
use crate::atp::event_emitter::EventEmitter;
use crate::atp::types::AgentStatus;
use crate::config::ClientConfig;
use crate::error::ClientError;
use crate::notification::NotificationStore;
use crate::persistence;
use crate::server::ServerHandle;

/// Shared application state. Both clients wrap this in Arc<RwLock<AppState>>.
pub struct AppState {
    pub config: ClientConfig,
    pub dispatcher: Option<Arc<Dispatcher>>,
    pub emitter: Option<EventEmitter>,
    pub notifications: Arc<NotificationStore>,
    pub agents: Arc<RwLock<Vec<ActiveAgent>>>,
    pub connection_status: ConnectionStatus,
    pub server_handle: Option<ServerHandle>,
    pub pending_hils: Arc<RwLock<HashMap<String, (u64, String)>>>, // hil_id -> (pid, approval_token)
    pub emit_callback: Option<EmitCallback>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("dispatcher", &self.dispatcher.is_some())
            .field("emitter", &self.emitter.is_some())
            .field("notifications", &self.notifications)
            .field("agents", &self.agents)
            .field("connection_status", &self.connection_status)
            .field("server_handle", &self.server_handle.is_some())
            .field("pending_hils", &self.pending_hils)
            .field("emit_callback", &self.emit_callback.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected { session_id: String },
    Reconnecting { attempt: u32 },
}

impl ConnectionStatus {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            ConnectionStatus::Connected { session_id } => Some(session_id),
            _ => None,
        }
    }
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
            server_handle: None,
            pending_hils: Arc::new(RwLock::new(HashMap::new())),
            emit_callback: None,
        }
    }

    pub async fn init(&mut self) -> Result<(), ClientError> {
        // Ensure server is running
        self.server_handle = Some(ServerHandle::ensure_running(&self.config).await?);

        // Load persisted notifications
        if let Ok(persisted) = persistence::load_notifications() {
            for n in persisted {
                self.notifications.add(n).await;
            }
        }

        // TODO: Connect to server and start emitter

        Ok(())
    }

    pub fn set_emit_callback(&mut self, callback: EmitCallback) {
        self.emit_callback = Some(callback);
        // Start the event bridge if emitter exists
        if let Some(_emitter) = &self.emitter {
            self.start_event_bridge();
        }
    }

    fn start_event_bridge(&self) {
        if let (Some(_emitter), Some(_callback)) = (&self.emitter, &self.emit_callback) {
            // TODO: start the bridge
        }
    }
}

/// Type alias used everywhere in both clients.
pub type SharedState = Arc<RwLock<AppState>>;

/// Callback for emitting events to frontend.
pub type EmitCallback = Box<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

pub fn new_shared(config: ClientConfig) -> SharedState {
    Arc::new(RwLock::new(AppState::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_shared_starts_disconnected() {
        let cfg = ClientConfig::default();
        let state = new_shared(cfg);
        let s = state.try_read().unwrap();
        assert_eq!(s.connection_status, ConnectionStatus::Disconnected);
        assert!(s.dispatcher.is_none());
    }
}
