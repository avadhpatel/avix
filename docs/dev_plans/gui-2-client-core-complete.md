# GUI App Gap 2: Complete avix-client-core Shared Crate

## Spec Reference
docs/spec/gui-cli-via-atp.md sections:
* s3 Shared Library `avix-client-core`: src/ lib.rs, config.rs, server.rs, atp/{client.rs, types.rs, dispatcher.rs, event_emitter.rs, notification.rs}, state.rs, persistence.rs, commands.rs.
* s6 Unified Flows: config init, server start, ATP connect/reconnect, agent spawn, streaming, HIL, notifications.
* s7 Persistence: appDataDir/notifications.json, ui-layout.json (atomic).
* s9 Cargo.toml: avix-client-core deps (avix-core/protocol, tokio-tungstenite).

## Goals
* Create complete shared lib for GUI/CLI: config, server mgmt, ATP protocol handling, state, persistence.
* Define all ATP types (Cmd/Reply/Event/HilRequest/Notification).
* Implement WS client w/ reconnect, dispatcher, event emitter, HIL/notification logic.
* Achieve high test coverage for lib.

## Dependencies
* avix-core, avix-protocol.
* tokio 1.x full, tokio-tungstenite 0.24 rustls-tls, serde derive, dirs (appDataDir), tempfile (tests).

## Files to Create/Edit
* Cargo.toml (root): add \"crates/avix-client-core\" to members.
* crates/avix-client-core/Cargo.toml
* crates/avix-client-core/src/lib.rs (pub use all modules, AppState)
* crates/avix-client-core/src/config.rs
* crates/avix-client-core/src/server.rs
* crates/avix-client-core/src/atp/client.rs
* crates/avix-client-core/src/atp/types.rs
* crates/avix-client-core/src/atp/dispatcher.rs
* crates/avix-client-core/src/atp/event_emitter.rs
* crates/avix-client-core/src/atp/notification.rs
* crates/avix-client-core/src/state.rs
* crates/avix-client-core/src/persistence.rs
* crates/avix-client-core/src/commands.rs
* crates/avix-client-core/tests/* (integration)

## Detailed Tasks
1. Root Cargo.toml: add avix-client-core to members (after avix-app).

2. crates/avix-client-core/Cargo.toml:
```
toml
[package]
name = \"avix-client-core\"
version = \"0.1.0\"
edition = \"2021\"

[dependencies]
avix-core = { path = \"../avix-core\" }
avix-protocol = { path = \"../avix-protocol\" }
tokio = { version = \"1\", features = [\"full\"] }
tokio-tungstenite = { version = \"0.24\", features = [\"rustls-tls-webpki-roots\"] }
serde = { version = \"1\", features = [\"derive\"] }
dirs = \"5\"
anyhow = \"1\"
tracing = \"0.1\"
```
   * Optional feature `tauri` for GUI-specific if needed.

3. src/atp/types.rs: ATP structs from avix-protocol + client-specific:
```
rust
#[derive(Serialize, Deserialize)]
pub struct Cmd { domain: String, op: String, params: serde_json::Value }

pub struct Reply { success: bool, data: Option<Value>, error: Option<String> }

pub struct Event { agent_id: Uuid, session_id: Option<Uuid>, kind: EventKind, content: Value }

pub enum EventKind { Output, Status }

#[derive(Serialize, Deserialize, Clone)]
pub struct HilRequest {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub prompt: String,
    pub context: Vec<String>,
}

#[derive(Clone)]
pub struct Notification {
    pub id: Uuid,
    pub kind: NotificationKind,  // Hil | AgentExit | SysAlert
    pub title: String,
    pub body: String,
    pub timestamp: DateTime<Utc>,
}
```
   * Exact fields from spec s3 Key types.

4. src/atp/client.rs: tokio-tungstenite WS to ws://localhost:9142/atp, reconnect every 60s.
   * connect(), send_cmd(Cmd) -> Reply, listen_events() -> EventEmitter.

5. src/atp/dispatcher.rs: route Cmd by domain/op -> handler -> Reply.
   * proc.spawn -> atp proc.spawn etc.

6. src/atp/event_emitter.rs: parse WS frames -> typed Event/Notification emit (tokio channel).

7. src/atp/notification.rs: Vec<Notification> store, add_hil(HilRequest), resolve_hil(id, approve:bool).

8. src/config.rs: wrap avix-core::Config, init/load/save.

9. src/server.rs: spawn \"avix start\" or RuntimeExecutor, monitor health.

10. src/state.rs: AppState(Arc<Mutex<AppStateInner>>): Client, Notifications, Config, Layout.

11. src/persistence.rs: app_data_dir(), save/load notifications.json/ui-layout.json atomic (fs::rename).

12. src/commands.rs: async fn spawn_agent(name: String, desc: String) -> Result<Uuid>, resolve_hil(id: Uuid, approve: bool).

13. src/lib.rs: pub type AppState = Arc<AppStateInner>; pub use *;

14. Tests: #[tokio::test] mock WS/dispatcher, tempfile::tempdir for persistence, 95% cov.

## Verify
* `cargo test --package avix-client-core` passes 95% cov.
* `cargo check` in workspace.
* CLI prototype (future) uses it unchanged; lib self-contained.

Est: 4h