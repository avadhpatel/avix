# Client Gap A — `avix-client-core` Crate Scaffold + ATP Types

> **Status:** Pending
> **Priority:** Critical — all other client gaps depend on this
> **Depends on:** `avix-core`, `avix-protocol` (existing)
> **Blocks:** Client gaps B through H
> **Affects:** `Cargo.toml` (workspace), new crate `crates/avix-client-core/`

---

## Problem

There is no shared library for client-side ATP protocol handling, config management, or
server control. Both `avix-app` and `avix-cli` will need the same ATP types, WebSocket
client, notification store, and AppState. Without a shared crate, this logic gets
duplicated in two binaries.

---

## Scope

Create the `avix-client-core` crate skeleton and define all ATP wire types. No network
code yet — just the crate structure, `Cargo.toml`, module layout, and the complete set
of serialisable ATP structs.

---

## What Needs to Be Built

### 1. Workspace wiring — root `Cargo.toml`

Add `"crates/avix-client-core"` to the workspace `members` array.

### 2. `crates/avix-client-core/Cargo.toml`

```toml
[package]
name    = "avix-client-core"
version.workspace = true
edition.workspace = true

[dependencies]
avix-core      = { path = "../avix-core" }
tokio          = { workspace = true }
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
serde          = { workspace = true }
serde_json     = { workspace = true }
thiserror      = { workspace = true }
tracing        = { workspace = true }
uuid           = { workspace = true }
reqwest        = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }

[dev-dependencies]
tokio          = { workspace = true, features = ["full"] }
```

### 3. Module layout — `src/lib.rs`

```
avix-client-core/
└── src/
    ├── lib.rs             ← re-exports public surface + error type
    ├── error.rs           ← ClientError (thiserror)
    ├── atp/
    │   ├── mod.rs
    │   ├── types.rs       ← ALL ATP structs (this gap)
    │   ├── client.rs      ← gap B
    │   ├── dispatcher.rs  ← gap B
    │   └── event_emitter.rs ← gap C
    ├── notification.rs    ← gap D
    ├── persistence.rs     ← gap D
    ├── state.rs           ← gap E
    ├── config.rs          ← gap E
    ├── server.rs          ← gap E
    └── commands.rs        ← gap E
```

`lib.rs` should `pub mod atp; pub mod error;` and stub the rest as `pub mod …` with
`todo!()` bodies so the crate compiles from day one.

### 4. `error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ATP error {code}: {message}")]
    Atp { code: String, message: String },
    #[error("Not connected")]
    NotConnected,
    #[error("Timeout")]
    Timeout,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
```

### 5. `atp/types.rs` — Complete ATP type surface

All types must derive `Debug, Clone, Serialize, Deserialize`.

#### 5a. Outgoing — `Cmd`

```rust
/// An outgoing ATP command frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cmd {
    #[serde(rename = "type")]
    pub frame_type: String,   // always "cmd"
    pub domain: String,
    pub op: String,
    pub id: String,
    pub token: String,
    pub body: serde_json::Value,
}

impl Cmd {
    pub fn new(domain: &str, op: &str, token: &str, body: serde_json::Value) -> Self {
        Self {
            frame_type: "cmd".into(),
            domain: domain.into(),
            op: op.into(),
            id: uuid::Uuid::new_v4().to_string(),
            token: token.into(),
            body,
        }
    }
}

/// Subscribe frame sent after connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscribe {
    #[serde(rename = "type")]
    pub frame_type: String,   // always "subscribe"
    pub events: Vec<String>,  // e.g. ["*"]
}
```

#### 5b. Incoming — `Reply`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(rename = "type")]
    pub frame_type: String,   // always "reply"
    pub id: String,
    pub ok: bool,
    pub code: Option<String>,
    pub message: Option<String>,
    pub body: Option<serde_json::Value>,
}
```

#### 5c. Incoming — `Event` with typed payload

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub frame_type: String,   // always "event"
    pub kind: EventKind,
    pub owner_session: Option<String>,
    pub body: EventBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionReady,
    SessionClosing,
    TokenExpiring,
    AgentOutput,
    AgentStatus,
    AgentToolCall,
    AgentToolResult,
    AgentExit,
    ProcSignal,
    HilRequest,
    HilResolved,
    FsChanged,
    ToolChanged,
    CronFired,
    SysService,
    SysAlert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventBody {
    SessionReady(SessionReadyBody),
    AgentOutput(AgentOutputBody),
    AgentStatus(AgentStatusBody),
    AgentExit(AgentExitBody),
    HilRequest(HilRequestBody),
    HilResolved(HilResolvedBody),
    SysAlert(SysAlertBody),
    Raw(serde_json::Value),   // fallback for unrecognised / future kinds
}
```

#### 5d. Typed event body structs

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadyBody {
    pub session_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutputBody {
    pub pid: u64,
    pub session_id: String,
    pub text: String,
    pub turn: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusBody {
    pub pid: u64,
    pub session_id: String,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus { Running, Paused, Stopped, Crashed }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExitBody {
    pub pid: u64,
    pub session_id: String,
    pub exit_code: i32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilRequestBody {
    pub hil_id: String,
    pub pid: u64,
    pub session_id: String,
    pub approval_token: String,
    pub prompt: String,
    pub timeout_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilResolvedBody {
    pub hil_id: String,
    pub pid: u64,
    pub outcome: HilOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HilOutcome { Approved, Denied, Timeout }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysAlertBody {
    pub level: String,
    pub message: String,
}
```

#### 5e. `LoginRequest` / `LoginResponse`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub identity: String,
    pub credential: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}
```

#### 5f. Incoming frame discriminator

```rust
/// Parse any raw ATP text frame to the correct variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Frame {
    Reply(Reply),
    Event(Event),
}
```

---

## Tests (in `atp/types.rs` under `#[cfg(test)]`)

```rust
#[test]
fn cmd_serialises_correctly() {
    let cmd = Cmd::new("proc", "spawn", "tok-abc", serde_json::json!({"agent": "test"}));
    let s = serde_json::to_string(&cmd).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["type"], "cmd");
    assert_eq!(v["domain"], "proc");
    assert_eq!(v["op"], "spawn");
    assert!(v["id"].as_str().unwrap().len() > 0);
}

#[test]
fn reply_roundtrip() {
    let json = r#"{"type":"reply","id":"r1","ok":true,"body":{"pid":42}}"#;
    let r: Reply = serde_json::from_str(json).unwrap();
    assert!(r.ok);
    assert_eq!(r.id, "r1");
}

#[test]
fn event_hil_request_roundtrip() {
    let json = r#"{
        "type":"event","kind":"hil_request","owner_session":"sess-1",
        "body":{"hil_id":"h1","pid":10,"session_id":"sess-1",
                "approval_token":"tok","prompt":"approve?","timeout_secs":600}
    }"#;
    let e: Event = serde_json::from_str(json).unwrap();
    assert_eq!(e.kind, EventKind::HilRequest);
}

#[test]
fn frame_discriminator_reply() {
    let json = r#"{"type":"reply","id":"x","ok":false,"code":"EPARSE","message":"bad"}"#;
    let f: Frame = serde_json::from_str(json).unwrap();
    assert!(matches!(f, Frame::Reply(_)));
}

#[test]
fn event_kind_all_known_kinds_deserialise() {
    let kinds = [
        ("session_ready", EventKind::SessionReady),
        ("agent_output", EventKind::AgentOutput),
        ("hil_request", EventKind::HilRequest),
        ("sys_alert", EventKind::SysAlert),
    ];
    for (s, expected) in kinds {
        let v = format!("\"{}\"", s);
        let k: EventKind = serde_json::from_str(&v).unwrap();
        assert_eq!(k, expected);
    }
}
```

---

## Success Criteria

- [ ] `cargo build --workspace` succeeds (crate compiles with stub modules)
- [ ] `cargo test --workspace` passes all tests in `atp/types.rs`
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
- [ ] `cargo fmt --check` — zero diff
- [ ] No business logic beyond type definitions and the `ClientError` enum
