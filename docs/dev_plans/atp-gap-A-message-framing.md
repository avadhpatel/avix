# ATP Gap A — Message Framing & Wire Types

> **Spec reference:** §5 Message Framing, §9 Error Codes
> **Priority:** Critical — all other ATP gaps depend on this
> **Depends on:** —

---

## Problem

The existing `ATPCommand` / `ATPResponse` types use a `method`/`params` wire format that
does not match the ATP spec. The spec defines four distinct message kinds (`cmd`, `reply`,
`event`, `subscribe`) with specific fields. There are no typed error codes.

**Current state (`crates/avix-core/src/gateway/atp/`):**

```rust
// command.rs — wrong framing
pub enum ATPCommand {
    AgentSpawn { name: String, goal: String },
    ...
}
impl ATPCommand {
    pub fn from_json(value: &Value) -> Option<Self> { ... }  // uses "method"/"params"
}

// response.rs — missing type/error-code fields
pub struct ATPResponse {
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,  // plain string, no code
}
```

**What spec requires (§5):**

```json
// client → server: cmd
{ "type": "cmd", "id": "c-0042", "token": "<ATPToken>",
  "domain": "proc", "op": "spawn", "body": { ... } }

// server → client: reply
{ "type": "reply", "id": "c-0042", "ok": true, "body": { ... } }
{ "type": "reply", "id": "c-0042", "ok": false,
  "error": { "code": "EPERM", "message": "...", "detail": { ... } } }

// server → client: event
{ "type": "event", "event": "agent.output", "sessionId": "sess-alice-001",
  "ts": "2026-03-20T10:12:34.123Z", "body": { ... } }

// client → server: subscribe
{ "type": "subscribe", "id": "sub-001", "token": "<ATPToken>",
  "events": ["agent.output", "agent.status"] }
```

---

## What to Build

### 1. `AtpErrorCode` enum

File: `crates/avix-core/src/gateway/atp/error.rs`

```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AtpErrorCode {
    Eauth,      // 401 — invalid/missing token
    Eexpired,   // 401 — token expired
    Esession,   // 401 — session ID mismatch
    Eperm,      // 403 — insufficient role
    Enotfound,  // 404 — target doesn't exist
    Econflict,  // 409 — operation conflicts with current state
    Eused,      // 409 — ApprovalToken already consumed
    Elimit,     // 429 — quota exceeded
    Eparse,     // 400 — malformed message
    Einternal,  // 500 — kernel-side error
    Eunavail,   // 503 — service not running
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpError {
    pub code: AtpErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl AtpError {
    pub fn new(code: AtpErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), detail: None }
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

#[derive(Debug, Error)]
pub enum AtpFrameError {
    #[error("malformed frame: {0}")]
    Parse(String),
    #[error("unknown message type: {0}")]
    UnknownType(String),
}
```

### 2. `AtpDomain` and `AtpEventKind` enums

File: `crates/avix-core/src/gateway/atp/types.rs`

```rust
use serde::{Deserialize, Serialize};

/// The 11 command domains in the ATP spec (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AtpDomain {
    Auth,
    Proc,
    Signal,
    Fs,
    Snap,
    Cron,
    Users,
    Crews,
    Cap,
    Sys,
    Pipe,
}

/// All 18 server-push event kinds (§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtpEventKind {
    #[serde(rename = "session.ready")]     SessionReady,
    #[serde(rename = "session.closing")]   SessionClosing,
    #[serde(rename = "token.expiring")]    TokenExpiring,
    #[serde(rename = "agent.output")]      AgentOutput,
    #[serde(rename = "agent.status")]      AgentStatus,
    #[serde(rename = "agent.tool_call")]   AgentToolCall,
    #[serde(rename = "agent.tool_result")] AgentToolResult,
    #[serde(rename = "agent.exit")]        AgentExit,
    #[serde(rename = "proc.signal")]       ProcSignal,
    #[serde(rename = "hil.request")]       HilRequest,
    #[serde(rename = "hil.resolved")]      HilResolved,
    #[serde(rename = "fs.changed")]        FsChanged,
    #[serde(rename = "tool.changed")]      ToolChanged,
    #[serde(rename = "cron.fired")]        CronFired,
    #[serde(rename = "sys.service")]       SysService,
    #[serde(rename = "sys.alert")]         SysAlert,
}
```

### 3. Four wire message types + `AtpFrame` dispatch enum

File: `crates/avix-core/src/gateway/atp/frame.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use super::error::{AtpError, AtpFrameError};
use super::types::{AtpDomain, AtpEventKind};

/// A command sent from client → gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpCmd {
    #[serde(rename = "type")]
    pub msg_type: String,     // always "cmd"
    pub id: String,
    pub token: String,
    pub domain: AtpDomain,
    pub op: String,
    pub body: Value,
}

/// A reply sent from gateway → client, correlated by id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpReply {
    #[serde(rename = "type")]
    pub msg_type: String,     // always "reply"
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AtpError>,
}

impl AtpReply {
    pub fn ok(id: impl Into<String>, body: Value) -> Self {
        Self { msg_type: "reply".into(), id: id.into(), ok: true,
               body: Some(body), error: None }
    }
    pub fn err(id: impl Into<String>, error: AtpError) -> Self {
        Self { msg_type: "reply".into(), id: id.into(), ok: false,
               body: None, error: Some(error) }
    }
}

/// A server-push event sent from gateway → client (no reply expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpEvent {
    #[serde(rename = "type")]
    pub msg_type: String,     // always "event"
    pub event: AtpEventKind,
    pub session_id: String,
    pub ts: DateTime<Utc>,
    pub body: Value,
}

impl AtpEvent {
    pub fn new(event: AtpEventKind, session_id: impl Into<String>, body: Value) -> Self {
        Self { msg_type: "event".into(), event, session_id: session_id.into(),
               ts: Utc::now(), body }
    }
}

/// A subscription request sent from client → gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpSubscribe {
    #[serde(rename = "type")]
    pub msg_type: String,     // always "subscribe"
    pub id: String,
    pub token: String,
    pub events: Vec<String>,  // event names or ["*"]
}

/// Top-level dispatch enum for inbound frames.
#[derive(Debug, Clone)]
pub enum AtpFrame {
    Cmd(AtpCmd),
    Subscribe(AtpSubscribe),
}

impl AtpFrame {
    /// Parse a raw JSON text frame into an `AtpFrame`.
    pub fn parse(raw: &str) -> Result<Self, AtpFrameError> {
        let v: Value = serde_json::from_str(raw)
            .map_err(|e| AtpFrameError::Parse(e.to_string()))?;
        let msg_type = v["type"]
            .as_str()
            .ok_or_else(|| AtpFrameError::Parse("missing 'type' field".into()))?;
        match msg_type {
            "cmd" => {
                let cmd: AtpCmd = serde_json::from_value(v)
                    .map_err(|e| AtpFrameError::Parse(e.to_string()))?;
                Ok(AtpFrame::Cmd(cmd))
            }
            "subscribe" => {
                let sub: AtpSubscribe = serde_json::from_value(v)
                    .map_err(|e| AtpFrameError::Parse(e.to_string()))?;
                Ok(AtpFrame::Subscribe(sub))
            }
            other => Err(AtpFrameError::UnknownType(other.to_string())),
        }
    }
}
```

### 4. Update `mod.rs` exports

File: `crates/avix-core/src/gateway/atp/mod.rs`

```rust
pub mod command;     // keep for now (will migrate in Gap E)
pub mod error;
pub mod frame;
pub mod response;   // keep for now
pub mod types;

pub use error::{AtpError, AtpErrorCode, AtpFrameError};
pub use frame::{AtpCmd, AtpEvent, AtpFrame, AtpReply, AtpSubscribe};
pub use types::{AtpDomain, AtpEventKind};
// legacy re-exports (remove when Gap E lands)
pub use command::ATPCommand;
pub use response::ATPResponse;
```

---

## Tests to Write

File: `crates/avix-core/src/gateway/atp/frame.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::error::AtpErrorCode;

    #[test]
    fn parse_cmd_frame() {
        let raw = r#"{
            "type": "cmd", "id": "c-001", "token": "tok",
            "domain": "proc", "op": "spawn",
            "body": { "agent": "researcher" }
        }"#;
        let frame = AtpFrame::parse(raw).unwrap();
        match frame {
            AtpFrame::Cmd(cmd) => {
                assert_eq!(cmd.id, "c-001");
                assert_eq!(cmd.domain, AtpDomain::Proc);
                assert_eq!(cmd.op, "spawn");
            }
            _ => panic!("expected Cmd"),
        }
    }

    #[test]
    fn parse_subscribe_frame() {
        let raw = r#"{
            "type": "subscribe", "id": "sub-001", "token": "tok",
            "events": ["agent.output", "agent.status"]
        }"#;
        let frame = AtpFrame::parse(raw).unwrap();
        assert!(matches!(frame, AtpFrame::Subscribe(_)));
    }

    #[test]
    fn parse_unknown_type_returns_error() {
        let raw = r#"{ "type": "unknown" }"#;
        assert!(matches!(AtpFrame::parse(raw), Err(AtpFrameError::UnknownType(_))));
    }

    #[test]
    fn parse_missing_type_returns_error() {
        let raw = r#"{ "id": "x" }"#;
        assert!(matches!(AtpFrame::parse(raw), Err(AtpFrameError::Parse(_))));
    }

    #[test]
    fn reply_ok_serializes_correctly() {
        let reply = AtpReply::ok("c-001", serde_json::json!({ "pid": 42 }));
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains("\"type\":\"reply\""));
        assert!(s.contains("\"ok\":true"));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn reply_err_serializes_correctly() {
        let err = AtpError::new(AtpErrorCode::Eperm, "not allowed");
        let reply = AtpReply::err("c-002", err);
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("EPERM"));
    }

    #[test]
    fn event_has_ts_and_session_id() {
        let ev = AtpEvent::new(
            AtpEventKind::AgentOutput,
            "sess-001",
            serde_json::json!({ "text": "hello" }),
        );
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"event\""));
        assert!(s.contains("sess-001"));
        assert!(s.contains("agent.output"));
    }

    #[test]
    fn error_code_round_trips() {
        let code = AtpErrorCode::Eused;
        let s = serde_json::to_string(&code).unwrap();
        assert_eq!(s, "\"EUSED\"");
        let back: AtpErrorCode = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AtpErrorCode::Eused);
    }

    #[test]
    fn all_domains_round_trip() {
        for domain in [
            AtpDomain::Auth, AtpDomain::Proc, AtpDomain::Signal, AtpDomain::Fs,
            AtpDomain::Snap, AtpDomain::Cron, AtpDomain::Users, AtpDomain::Crews,
            AtpDomain::Cap, AtpDomain::Sys, AtpDomain::Pipe,
        ] {
            let s = serde_json::to_string(&domain).unwrap();
            let back: AtpDomain = serde_json::from_str(&s).unwrap();
            assert_eq!(back, domain);
        }
    }
}
```

---

## Success Criteria

- [ ] `AtpFrame::parse` correctly dispatches `cmd` and `subscribe` frames
- [ ] `AtpReply::ok` / `::err` serialize to spec-compliant JSON
- [ ] `AtpEvent::new` populates `type`, `event`, `sessionId`, `ts`, `body`
- [ ] All 11 `AtpDomain` variants round-trip through serde
- [ ] All 11 `AtpErrorCode` variants serialize to `SCREAMING_SNAKE_CASE`
- [ ] All 16 `AtpEventKind` variants serialize to dot-separated names
- [ ] `cargo test --workspace` passes, `cargo clippy` zero warnings
