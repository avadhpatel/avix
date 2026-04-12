use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;
use tracing::warn;

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
        // Ensure server is running (best-effort — client apps don't own the daemon lifecycle).
        if let Ok(handle) = ServerHandle::ensure_running(&self.config).await {
            self.server_handle = Some(handle);
        } else {
            warn!("Failed to start server in init");
        }

        // Load persisted notifications.
        if let Ok(persisted) = persistence::load_notifications() {
            for n in persisted {
                self.notifications.add(n).await;
            }
            debug!(
                "State update agents=0 notifs={} hil=0",
                self.notifications.unread_count().await
            );
        }

        // Connect only when a credential is present.  If it's missing (fresh
        // install, or config not yet written), leave the state as Disconnected
        // so the UI can show a login page instead of crashing.
        if !self.config.credential.is_empty() {
            if let Err(e) = self.do_connect().await {
                warn!("Initial connect failed (will show login): {}", e);
            }
        } else {
            debug!("No credential configured — skipping initial connect");
        }

        Ok(())
    }

    /// Authenticate with explicit credentials, update the in-memory config, and
    /// establish the dispatcher + event bridge.  Does **not** persist the config
    /// to disk — callers decide whether to save.
    pub async fn login(&mut self, identity: &str, credential: &str) -> Result<(), ClientError> {
        self.config.identity = identity.to_string();
        self.config.credential = credential.to_string();
        self.do_connect().await
    }

    /// Returns true when a dispatcher is active (i.e. we are authenticated).
    pub fn is_authenticated(&self) -> bool {
        self.dispatcher.is_some()
    }

    async fn do_connect(&mut self) -> Result<(), ClientError> {
        self.connection_status = ConnectionStatus::Connecting;

        let client = match AtpClient::connect(self.config.clone()).await {
            Ok(c) => c,
            Err(e) => {
                warn!("ATP connect failed: {}", e);
                self.connection_status = ConnectionStatus::Disconnected;
                return Err(ClientError::Other(e.into()));
            }
        };

        let dispatcher = Arc::new(Dispatcher::new(client));
        self.dispatcher = Some(dispatcher);
        self.connection_status = ConnectionStatus::Connected {
            session_id: "core-init".to_string(),
        };

        // If a UI callback was registered before login completed, start the
        // event bridge now that the dispatcher exists.
        if self.emit_callback.is_some() {
            self.start_event_bridge();
        }

        debug!("ATP connected, session ready");
        Ok(())
    }

    pub fn set_emit_callback(&mut self, callback: EmitCallback) {
        self.emit_callback = Some(callback);
        if self.dispatcher.is_some() {
            self.start_event_bridge();
        }
    }

    fn start_event_bridge(&self) {
        if let (Some(dispatcher), Some(callback)) = (&self.dispatcher, &self.emit_callback) {
            let mut rx = dispatcher.events();
            let callback = callback.clone();
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let event_name: &str = match event.kind {
                        crate::atp::types::EventKind::SessionReady => "daemon-ready",
                        crate::atp::types::EventKind::AgentSpawned => "agent.spawned",
                        crate::atp::types::EventKind::AgentOutput => "agent.output",
                        crate::atp::types::EventKind::AgentOutputChunk => "agent.output.chunk",
                        crate::atp::types::EventKind::AgentStatus => "agent.status",
                        crate::atp::types::EventKind::AgentExit => "agent.exit",
                        crate::atp::types::EventKind::ToolChanged => "tool.changed",
                        crate::atp::types::EventKind::SysService => "sys.service",
                        crate::atp::types::EventKind::AgentToolCall => "agent.tool_call",
                        crate::atp::types::EventKind::AgentToolResult => "agent.tool_result",
                        _ => continue,
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
