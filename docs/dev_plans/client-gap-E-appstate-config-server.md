# Client Gap E — AppState, Config Wrap, Server Control + High-Level Commands

> **Status:** Pending
> **Priority:** High
> **Depends on:** Client gaps B, C, D
> **Blocks:** Client gaps F, G, H (clients need AppState to share context)
> **Affects:** `crates/avix-client-core/src/state.rs`,
>   `crates/avix-client-core/src/config.rs`,
>   `crates/avix-client-core/src/server.rs`,
>   `crates/avix-client-core/src/commands.rs`

---

## Problem

Both GUI and CLI need to share a single `AppState` struct that holds the ATP connection,
notification store, active agent list, and connection status. They also both need to
start/stop the Avix server process, load client-side config, and issue high-level
commands (spawn agent, send signal, etc.) without duplicating the ATP call sequences.

---

## Scope

Implement `AppState`, `ClientConfig`, `ServerHandle`, and the `commands` module. These
are pure Rust — no Tauri, no Ratatui. Both clients call these functions directly.

---

## What Needs to Be Built

### 1. `state.rs`

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::atp::dispatcher::Dispatcher;
use crate::atp::event_emitter::EventEmitter;
use crate::notification::NotificationStore;

/// Shared application state. Both clients wrap this in Arc<RwLock<AppState>>.
#[derive(Debug)]
pub struct AppState {
    pub config: crate::config::ClientConfig,
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
    pub status: crate::atp::types::AgentStatus,
    pub goal: String,
}

impl AppState {
    pub fn new(config: crate::config::ClientConfig) -> Self { … }
}

/// Type alias used everywhere in both clients.
pub type SharedState = Arc<RwLock<AppState>>;

pub fn new_shared(config: crate::config::ClientConfig) -> SharedState {
    Arc::new(RwLock::new(AppState::new(config)))
}
```

---

### 2. `config.rs`

Client-side configuration — distinct from `avix-core`'s server config. This tells the
client where to connect, which identity to use, and where to find the runtime root.

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    /// e.g. "http://127.0.0.1:7700"
    pub server_url: String,
    pub identity: String,
    /// API key or password for ATP login.
    pub credential: String,
    /// Runtime root, used to spawn the server if not already running.
    pub runtime_root: PathBuf,
    /// Auto-start the server if not reachable on startup.
    #[serde(default = "default_true")]
    pub auto_start_server: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:7700".into(),
            identity: "admin".into(),
            credential: String::new(),
            runtime_root: dirs::home_dir()
                .unwrap_or_default()
                .join("avix-data"),
            auto_start_server: true,
        }
    }
}

fn default_true() -> bool { true }

impl ClientConfig {
    /// Load from `{app_data_dir}/client.json`, or return `Default` if missing.
    pub fn load() -> Result<Self, crate::error::ClientError> { … }

    /// Save to `{app_data_dir}/client.json`.
    pub fn save(&self) -> Result<(), crate::error::ClientError> { … }
}
```

---

### 3. `server.rs`

Spawn and monitor the `avix start` process. The client should be able to start the
server if `auto_start_server` is true and the server is not already listening.

```rust
use std::process::Child;
use std::sync::{Arc, Mutex};
use crate::config::ClientConfig;
use crate::error::ClientError;

pub struct ServerHandle {
    child: Arc<Mutex<Option<Child>>>,
}

impl ServerHandle {
    /// Probe the server URL — if not reachable, spawn `avix start`.
    /// Returns immediately if the server is already running.
    pub async fn ensure_running(config: &ClientConfig) -> Result<Self, ClientError> { … }

    /// True if the child process we spawned is still alive.
    pub fn is_alive(&self) -> bool { … }

    /// Kill the child process (if we own it).
    pub fn stop(&self) -> Result<(), ClientError> { … }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Do NOT kill the server on drop — the server outlives the client.
    }
}
```

Probe logic: `GET {server_url}/atp/health` (or `/atp/auth/login` with dummy creds —
expect 401, not connection refused). If connection refused → spawn. Retry probe up to
5 times with 500ms sleep before declaring the spawn failed.

---

### 4. `commands.rs`

High-level async functions that assemble `Cmd` bodies and call `dispatcher.call(…)`.
These are not Tauri-specific — both clients call them.

```rust
use crate::atp::types::{Cmd, Reply};
use crate::atp::dispatcher::Dispatcher;
use crate::error::ClientError;

/// Spawn a new agent.
pub async fn spawn_agent(
    dispatcher: &Dispatcher,
    token: &str,
    agent: &str,
    goal: &str,
    capabilities: &[&str],
) -> Result<u64, ClientError> {
    // Build proc.spawn Cmd, call, extract pid from reply body.
    …
}

/// Send a signal to an agent.
pub async fn send_signal(
    dispatcher: &Dispatcher,
    token: &str,
    pid: u64,
    signal: &str,
    payload: Option<serde_json::Value>,
) -> Result<(), ClientError> { … }

/// Send SIGPIPE with a text payload.
pub async fn pipe_text(
    dispatcher: &Dispatcher,
    token: &str,
    pid: u64,
    text: &str,
) -> Result<(), ClientError> {
    send_signal(dispatcher, token, pid, "SIGPIPE",
        Some(serde_json::json!({"text": text}))).await
}

/// Respond to a HIL request.
pub async fn resolve_hil(
    dispatcher: &Dispatcher,
    token: &str,
    pid: u64,
    hil_id: &str,
    approval_token: &str,
    approved: bool,
    note: Option<&str>,
) -> Result<(), ClientError> { … }

/// List active processes (proc.list).
pub async fn list_agents(
    dispatcher: &Dispatcher,
    token: &str,
) -> Result<Vec<serde_json::Value>, ClientError> { … }
```

---

## Tests

```rust
#[cfg(test)]
mod tests {
    // state.rs
    #[test]
    fn new_shared_starts_disconnected() {
        let cfg = ClientConfig::default();
        let state = new_shared(cfg);
        let s = state.try_read().unwrap();
        assert_eq!(s.connection_status, ConnectionStatus::Disconnected);
        assert!(s.dispatcher.is_none());
    }

    // config.rs
    #[test]
    fn client_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // patch app_data_dir to use dir.path() in test
        let cfg = ClientConfig {
            server_url: "http://localhost:7700".into(),
            identity: "bob".into(),
            credential: "secret".into(),
            runtime_root: dir.path().to_path_buf(),
            auto_start_server: false,
        };
        // save_json directly to avoid env patching
        let path = dir.path().join("client.json");
        crate::persistence::save_json(&path, &cfg).unwrap();
        let loaded: ClientConfig = crate::persistence::load_json(&path).unwrap();
        assert_eq!(loaded.identity, "bob");
        assert!(!loaded.auto_start_server);
    }

    // commands.rs — unit test with fake dispatcher from gap B test helpers
    #[tokio::test]
    async fn spawn_agent_extracts_pid_from_reply() {
        // Inject reply: { "ok": true, "body": { "pid": 42 } }
        // Call spawn_agent(...)
        // Assert returned pid == 42
    }

    #[tokio::test]
    async fn spawn_agent_returns_error_on_eperm() {
        // Inject reply: { "ok": false, "code": "EPERM" }
        // Assert ClientError::Atp { code: "EPERM", … }
    }

    #[tokio::test]
    async fn resolve_hil_sends_sigresume() {
        // Inject ok reply
        // Capture outbound cmd JSON
        // Assert domain=="signal", op=="send", body.signal=="SIGRESUME"
    }
}
```

---

## Dependencies to add to `avix-client-core/Cargo.toml`

```toml
dirs = "5"
```

---

## Success Criteria

- [ ] `AppState::new` initialises with `Disconnected` status and empty agent list
- [ ] `ClientConfig::load` returns `Default` when file absent
- [ ] `ServerHandle::ensure_running` spawns only when server is unreachable
- [ ] `commands::spawn_agent` extracts `pid` from reply body
- [ ] `commands::resolve_hil` sends `SIGRESUME` with correct payload
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
