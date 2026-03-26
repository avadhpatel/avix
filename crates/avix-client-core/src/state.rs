use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;
use tracing::warn;
use serde_json;

use crate::atp::types::AgentStatus;
use crate::atp::{AtpClient, Dispatcher, EventEmitter};
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
        if let Ok(handle) = ServerHandle::ensure_running(&self.config).await {
            self.server_handle = Some(handle);
        } else {
            warn!("Failed to start server in init");
        }

        // Load persisted notifications
        if let Ok(persisted) = persistence::load_notifications() {
            for n in persisted {
                self.notifications.add(n).await;
            }
            let agents_len = 0; // no agents loaded yet
            let notifs_count = self.notifications.unread_count().await;
            let hil_pending = 0; // no hil loaded yet
            debug!(
                "State update agents={} notifs={} hil={}",
                agents_len, notifs_count, hil_pending
            );
        }

        debug!(
            "State init: connecting to ATP server {}",
            self.config.server_url
        );

        self.connection_status = ConnectionStatus::Connecting;

        let client = AtpClient::connect(self.config.clone()).await.map_err(|e| {
            warn!("Failed to connect in init: {}", e);
            ClientError::Other(e.into())
        })?;

        let dispatcher = Arc::new(Dispatcher::new(client));
        let dispatcher_c = Arc::clone(&dispatcher);
        let emitter = EventEmitter::start(move || {
            let d = Arc::clone(&dispatcher_c);
            async move { Ok(Arc::try_unwrap(d).unwrap()) }
        });

        self.dispatcher = Some(dispatcher);
        self.emitter = Some(emitter);
        self.connection_status = ConnectionStatus::Connected {
            session_id: "core-init".to_string(),
        };

        debug!("State init complete, connected");

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
        if let (Some(emitter), Some(callback)) = (&self.emitter, &self.emit_callback) {
            let mut rx = emitter.subscribe_all();
            let callback = callback.clone(); // Clone the Arc to move into the spawn
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let event_name = match event.kind {
                        crate::atp::types::EventKind::SessionReady => "daemon-ready",
                        _ => continue, // Only emit daemon-ready for now
                    };
                    let data = serde_json::to_value(&event.body).unwrap_or(serde_json::Value::Null);
                    (callback)(event_name, &data);
                }
            });
        }
    }
}

/// Type alias used everywhere in both clients.
pub type SharedState = Arc<RwLock<AppState>>;

/// Callback for emitting events to frontend.
pub type EmitCallback = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;

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
