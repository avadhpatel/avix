# ATP Gap D — WebSocket Transport & HTTP Login

> **Spec reference:** §2 Transport Layer, §3.3 Obtaining a Token, §4 Connection Lifecycle
> **Priority:** Critical — nothing external can talk to Avix without this
> **Depends on:** ATP Gap A, Gap B, Gap C; IPC transport; AtpEventBus (Gap F can be stubbed)

---

## Problem

There is no running WebSocket server anywhere in the codebase. The spec requires:

- `POST /atp/auth/login` — HTTP endpoint returning an ATPToken
- `GET /atp` — WebSocket upgrade (Bearer token required)
- Two ports: 7700 (user operations) and 7701 (admin port, localhost-only)
- TLS 1.3 (self-signed cert auto-generated in dev)
- WebSocket PING every 30 s; close connection if no PONG within 10 s
- `session.ready` event pushed immediately after upgrade
- 60-second reconnect grace window — session state → idle; agents not stopped
- After grace window: session → closed, agents receive `SIGSTOP`

---

## What to Build

### 1. Add dependencies

`crates/avix-core/Cargo.toml`:

```toml
axum = { version = "0.7", features = ["ws"] }
axum-extra = { version = "0.9" }
tokio-tungstenite = { version = "0.21" }
rustls = { version = "0.23" }
rcgen = { version = "0.13" }          # self-signed cert generation
tokio-rustls = { version = "0.26" }
tower = { version = "0.4" }
```

### 2. `GatewayConfig`

File: `crates/avix-core/src/gateway/config.rs`

```rust
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Bind address for the user port (default: "127.0.0.1:7700")
    pub user_addr: std::net::SocketAddr,
    /// Bind address for the admin port (default: "127.0.0.1:7701")
    pub admin_addr: std::net::SocketAddr,
    /// TLS: if None, self-signed cert is auto-generated
    pub tls_cert_path: Option<std::path::PathBuf>,
    pub tls_key_path: Option<std::path::PathBuf>,
    /// Minimum TLS version ("1.2" or "1.3", default "1.3")
    pub tls_min_version: String,
    /// Allowed CORS origins (empty = localhost only)
    pub allowed_origins: Vec<String>,
    /// HIL timeout in seconds (default: 600)
    pub hil_timeout_secs: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            user_addr: "127.0.0.1:7700".parse().unwrap(),
            admin_addr: "127.0.0.1:7701".parse().unwrap(),
            tls_cert_path: None,
            tls_key_path: None,
            tls_min_version: "1.3".into(),
            allowed_origins: vec![],
            hil_timeout_secs: 600,
        }
    }
}
```

### 3. `ConnectionState` — per-WebSocket context

File: `crates/avix-core/src/gateway/connection.rs`

```rust
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use crate::auth::session::SessionState;
use crate::gateway::replay::ReplayGuard;

/// Shared state for one active WebSocket connection.
pub struct ConnectionState {
    pub session_id: String,
    pub is_admin_port: bool,
    pub replay_guard: ReplayGuard,
    /// Channel to send outbound frames (replies + events) to the WS writer task.
    pub outbound_tx: mpsc::Sender<String>,
}
```

### 4. `GatewayServer`

File: `crates/avix-core/src/gateway/server.rs`

Sketch (full axum routing):

```rust
use axum::{
    extract::{State, WebSocketUpgrade},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use crate::auth::{service::AuthService, atp_token::ATPTokenStore};
use crate::gateway::config::GatewayConfig;
use crate::gateway::event_bus::AtpEventBus;   // Gap F stub

pub struct GatewayServer {
    config: GatewayConfig,
    auth_svc: Arc<AuthService>,
    token_store: Arc<ATPTokenStore>,
    event_bus: Arc<AtpEventBus>,
}

impl GatewayServer {
    pub fn new(
        config: GatewayConfig,
        auth_svc: Arc<AuthService>,
        token_store: Arc<ATPTokenStore>,
        event_bus: Arc<AtpEventBus>,
    ) -> Self { ... }

    /// Starts both listener ports. Returns when either listener exits.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let user_app  = self.build_router(false);
        let admin_app = self.build_router(true);
        tokio::try_join!(
            axum::serve(
                tokio::net::TcpListener::bind(self.config.user_addr).await?,
                user_app,
            ),
            axum::serve(
                tokio::net::TcpListener::bind(self.config.admin_addr).await?,
                admin_app,
            ),
        )?;
        Ok(())
    }

    fn build_router(&self, is_admin_port: bool) -> Router {
        Router::new()
            .route("/atp/auth/login", post(handle_login))
            .route("/atp", get(handle_ws_upgrade))
            .with_state(AppState { server: Arc::clone(&self.server_arc), is_admin_port })
    }
}
```

#### `POST /atp/auth/login` handler

```
1. Parse JSON body: { "identity": "<name>", "credential": "<value>" }
2. Call AuthService::login(identity, credential)
3. Build ATPTokenClaims { sub, uid, role, crews, scope, sessionId, iat, exp }
4. Call ATPTokenStore::issue(claims) → token string
5. Return JSON: { "token": "<token>", "expiresAt": "<ISO>", "sessionId": "<id>" }
```

Error cases:
- Invalid credentials → 401 `{ "error": "EAUTH", "message": "invalid credential" }`
- Malformed body → 400

#### `GET /atp` WebSocket upgrade handler

```
1. Extract `Authorization: Bearer <token>` header
2. Validate token via ATPTokenStore::validate — reject 401 if invalid
3. Check session still valid in AuthService
4. axum WS upgrade
5. Spawn connection handler task
```

#### Connection handler task

```
Spawn two subtasks:
  A. Reader task — reads inbound WS frames
  B. Writer task — drains outbound_tx channel, sends WS frames

Reader task loop:
  1. Receive WS message
     - Text frame → AtpFrame::parse → dispatch to command handler
     - Ping → send Pong immediately
     - Pong → reset keep-alive timer
     - Close → break loop
  2. On AtpFrame::Cmd:
     a. Run AtpValidator::validate(cmd)
     b. On success → dispatch to domain handler → send reply via outbound_tx
     c. On error → send AtpReply::err via outbound_tx
  3. On AtpFrame::Subscribe → register event filters

Keep-alive task:
  - Tick every 30 s: send WS Ping
  - If no Pong within 10 s: close connection, session.mark_idle()

On connection drop:
  - session.mark_idle()  (start 60-second grace timer)
  - Spawn grace timer task:
      sleep 60 s
      if session still idle:
          session.mark_closed("reconnect timeout")
          send SIGSTOP to all session.agents
```

### 5. Immediately push `session.ready` on upgrade

After successful WS upgrade, before entering the read loop:

```rust
let ready_event = AtpEvent::new(
    AtpEventKind::SessionReady,
    &session_id,
    serde_json::json!({
        "sessionId": session_id,
        "identity": claims.sub,
        "role": claims.role,
    }),
);
outbound_tx.send(serde_json::to_string(&ready_event)?).await?;
```

### 6. `token.expiring` check after each validated command

After `AtpValidator::validate` succeeds, check:

```rust
if token_store.is_expiring_soon(&cmd.token).await? {
    let event = AtpEvent::new(AtpEventKind::TokenExpiring, &session_id, json!({
        "expiresAt": claims.exp,
        "remainingSeconds": (claims.exp - Utc::now()).num_seconds(),
    }));
    outbound_tx.send(serde_json::to_string(&event)?).await.ok();
}
```

---

## Domain Dispatcher Stub

File: `crates/avix-core/src/gateway/dispatcher.rs`

All domain handlers initially return `EUNAVAIL` stubs. They are implemented in Gap E.

```rust
pub async fn dispatch(
    cmd: ValidatedCmd,
    ipc_router: &IpcRouter,
) -> AtpReply {
    match cmd.cmd.domain {
        AtpDomain::Auth   => auth::handle(cmd, ipc_router).await,
        AtpDomain::Proc   => proc::handle(cmd, ipc_router).await,
        AtpDomain::Signal => signal::handle(cmd, ipc_router).await,
        AtpDomain::Fs     => fs::handle(cmd, ipc_router).await,
        AtpDomain::Snap   => snap::handle(cmd, ipc_router).await,
        AtpDomain::Cron   => cron::handle(cmd, ipc_router).await,
        AtpDomain::Users  => users::handle(cmd, ipc_router).await,
        AtpDomain::Crews  => crews::handle(cmd, ipc_router).await,
        AtpDomain::Cap    => cap::handle(cmd, ipc_router).await,
        AtpDomain::Sys    => sys::handle(cmd, ipc_router).await,
        AtpDomain::Pipe   => pipe::handle(cmd, ipc_router).await,
    }
}
```

---

## Tests to Write

File: `crates/avix-core/tests/gateway_transport.rs` (integration test)

```rust
/// These tests spin up a real GatewayServer on ephemeral ports
/// using a test-only plaintext (no-TLS) mode flag.

#[tokio::test]
async fn login_returns_token() {
    let srv = test_gateway().await;
    let resp = srv.post_login("alice", "sk-test").await;
    assert!(resp["token"].is_string());
    assert!(resp["sessionId"].is_string());
}

#[tokio::test]
async fn login_wrong_credential_returns_401() {
    let srv = test_gateway().await;
    let status = srv.post_login_status("alice", "wrong").await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn ws_upgrade_without_token_rejected() {
    let srv = test_gateway().await;
    let result = srv.ws_connect_no_auth().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ws_upgrade_sends_session_ready() {
    let srv = test_gateway().await;
    let token = srv.login("alice", "sk-test").await;
    let mut ws = srv.ws_connect(&token).await.unwrap();
    let first_msg = ws.recv_text().await.unwrap();
    let ev: serde_json::Value = serde_json::from_str(&first_msg).unwrap();
    assert_eq!(ev["type"], "event");
    assert_eq!(ev["event"], "session.ready");
}

#[tokio::test]
async fn cmd_with_invalid_token_returns_eauth() {
    let srv = test_gateway().await;
    let token = srv.login("alice", "sk-test").await;
    let mut ws = srv.ws_connect(&token).await.unwrap();
    ws.recv_text().await.unwrap(); // session.ready

    ws.send_cmd("c-001", "bad-token", "proc", "list", json!({})).await;
    let reply: serde_json::Value = serde_json::from_str(&ws.recv_text().await.unwrap()).unwrap();
    assert_eq!(reply["ok"], false);
    assert_eq!(reply["error"]["code"], "EAUTH");
}

#[tokio::test]
async fn duplicate_command_id_returns_eparse() {
    let srv = test_gateway().await;
    let token = srv.login("alice", "sk-test").await;
    let mut ws = srv.ws_connect(&token).await.unwrap();
    ws.recv_text().await.unwrap();

    ws.send_cmd("c-dup", &token, "proc", "list", json!({})).await;
    ws.recv_text().await.unwrap(); // first reply
    ws.send_cmd("c-dup", &token, "proc", "list", json!({})).await;
    let reply: serde_json::Value = serde_json::from_str(&ws.recv_text().await.unwrap()).unwrap();
    assert_eq!(reply["error"]["code"], "EPARSE");
}

#[tokio::test]
async fn admin_only_op_blocked_on_user_port() {
    let srv = test_gateway().await;
    let token = srv.login_admin("admin", "sk-admin").await;
    let mut ws = srv.ws_connect_user_port(&token).await.unwrap();
    ws.recv_text().await.unwrap();

    ws.send_cmd("c-001", &token, "cap", "grant", json!({})).await;
    let reply: serde_json::Value = serde_json::from_str(&ws.recv_text().await.unwrap()).unwrap();
    assert_eq!(reply["error"]["code"], "EPERM");
}
```

---

## Success Criteria

- [ ] `POST /atp/auth/login` returns `{ token, expiresAt, sessionId }` on valid credentials
- [ ] `POST /atp/auth/login` returns 401 on invalid credentials
- [ ] `GET /atp` without `Authorization` header rejects the upgrade
- [ ] `GET /atp` with valid token accepts the upgrade and pushes `session.ready`
- [ ] `session.ready` body contains `sessionId`, `identity`, `role`
- [ ] Inbound cmd with invalid token → `EAUTH` reply
- [ ] Duplicate command ID → `EPARSE` reply
- [ ] Admin-only op on user port (7700) → `EPERM` reply
- [ ] Keep-alive: server sends WS Ping every 30 s (observable via test timer)
- [ ] Session goes `idle` on disconnect; returns `active` on reconnect within 60 s
- [ ] `token.expiring` event pushed when < 5 min remain
- [ ] `cargo test --workspace`, `cargo clippy` pass
