use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use avix_core::gateway::atp::error::AtpError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cmd {
    #[serde(rename = "type")]
    pub frame_type: String, // always "cmd"
    pub domain: String,
    pub op: String,
    pub id: String,
    pub token: String,
    pub body: Value,
}

impl Cmd {
    pub fn new(domain: &str, op: &str, token: &str, body: Value) -> Self {
        Self {
            frame_type: "cmd".into(),
            domain: domain.into(),
            op: op.into(),
            id: Uuid::new_v4().to_string(),
            token: token.into(),
            body,
        }
    }
}

/// Subscribe frame sent after connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscribe {
    #[serde(rename = "type")]
    pub frame_type: String, // always "subscribe"
    pub events: Vec<String>, // e.g. ["*"]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    #[serde(rename = "type", skip_deserializing, default)]
    pub frame_type: String, // always "reply"
    pub id: String,
    pub ok: bool,
    pub code: Option<String>,
    pub message: Option<String>,
    pub body: Option<Value>,
    pub error: Option<AtpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type", skip_deserializing, default)]
    pub frame_type: String, // always "event"
    /// Wire field name is `"event"` (dot notation), e.g. `"agent.output"`.
    #[serde(rename = "event")]
    pub kind: EventKind,
    /// Wire field name is `"sessionId"` (camelCase).
    #[serde(rename = "sessionId")]
    pub owner_session: Option<String>,
    pub body: EventBody,
}

/// Event kind — matches the server's dot-notation wire format (e.g. `"agent.output"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventKind {
    #[serde(rename = "session.ready")]
    SessionReady,
    #[serde(rename = "session.closing")]
    SessionClosing,
    #[serde(rename = "token.expiring")]
    TokenExpiring,
    #[serde(rename = "agent.spawned")]
    AgentSpawned,
    #[serde(rename = "agent.output")]
    AgentOutput,
    #[serde(rename = "agent.status")]
    AgentStatus,
    #[serde(rename = "agent.tool_call")]
    AgentToolCall,
    #[serde(rename = "agent.tool_result")]
    AgentToolResult,
    #[serde(rename = "agent.exit")]
    AgentExit,
    #[serde(rename = "proc.start")]
    ProcStart,
    #[serde(rename = "proc.signal")]
    ProcSignal,
    #[serde(rename = "hil.request")]
    HilRequest,
    #[serde(rename = "hil.resolved")]
    HilResolved,
    #[serde(rename = "fs.changed")]
    FsChanged,
    #[serde(rename = "tool.changed")]
    ToolChanged,
    #[serde(rename = "cron.fired")]
    CronFired,
    #[serde(rename = "sys.service")]
    SysService,
    #[serde(rename = "sys.alert")]
    SysAlert,
    /// Incremental token delta from a streaming LLM turn.
    #[serde(rename = "agent.output.chunk")]
    AgentOutputChunk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventBody {
    SessionReady(SessionReadyBody),
    AgentSpawned(AgentSpawnedBody),
    AgentOutput(AgentOutputBody),
    AgentOutputChunk(AgentOutputChunkBody),
    AgentStatus(AgentStatusBody),
    AgentExit(AgentExitBody),
    HilRequest(HilRequestBody),
    HilResolved(HilResolvedBody),
    SysAlert(SysAlertBody),
    Raw(Value), // fallback for unrecognised / future kinds
}

impl EventBody {
    pub fn as_hil_request(&self) -> Option<&HilRequestBody> {
        match self {
            EventBody::HilRequest(body) => Some(body),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReadyBody {
    pub session_id: String,
    pub role: String,
}

/// Body for `agent.spawned` events.
/// `pid` is string-encoded to avoid u64 precision loss in JavaScript JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpawnedBody {
    pub pid: String,
    pub name: String,
    pub goal: String,
}

/// Body for `agent.output` events (full text, non-streaming).
/// `pid` is string-encoded to avoid u64 precision loss in JavaScript JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutputBody {
    pub pid: String,
    pub text: String,
}

/// Body for `agent.output.chunk` streaming events.
/// `pid` is string-encoded to avoid u64 precision loss in JavaScript JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutputChunkBody {
    pub pid: String,
    /// UUID correlating all chunks from one LLM turn.
    pub turn_id: String,
    pub text_delta: String,
    /// Monotonically increasing per turn for ordering / dedup.
    pub seq: u64,
    /// True on the last chunk of a turn.
    pub is_final: bool,
}

/// Body for `agent.status` events.
/// `pid` is string-encoded to avoid u64 precision loss in JavaScript JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusBody {
    pub pid: String,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    Running,
    Paused,
    Stopped,
    Crashed,
}

/// Body for `agent.exit` events.
/// `pid` is string-encoded to avoid u64 precision loss in JavaScript JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExitBody {
    pub pid: String,
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
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
pub enum HilOutcome {
    Approved,
    Denied,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysAlertBody {
    pub level: String,
    pub message: String,
}

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

/// Parse any raw ATP text frame to the correct variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Frame {
    Reply(Reply),
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilRequest {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub prompt: String,
    pub context: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationKind {
    Hil,
    AgentExit,
    SysAlert,
}

#[derive(Clone)]
pub struct Notification {
    pub id: Uuid,
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn cmd_serialises_correctly() {
        let cmd = Cmd::new(
            "proc",
            "spawn",
            "tok-abc",
            serde_json::json!({"agent": "test"}),
        );
        let s = serde_json::to_string(&cmd).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "cmd");
        assert_eq!(v["domain"], "proc");
        assert_eq!(v["op"], "spawn");
        assert!(!v["id"].as_str().unwrap().is_empty());
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
        // Wire format uses "event" field with dot notation, "sessionId" for session.
        let json = r#"{
            "type":"event","event":"hil.request","sessionId":"sess-1",
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
            ("session.ready", EventKind::SessionReady),
            ("agent.output", EventKind::AgentOutput),
            ("hil.request", EventKind::HilRequest),
            ("sys.alert", EventKind::SysAlert),
        ];
        for (s, expected) in kinds {
            let v = format!("\"{}\"", s);
            let k: EventKind = serde_json::from_str(&v).unwrap();
            assert_eq!(k, expected);
        }
    }

    #[test]
    fn hil_request_serialises() {
        let hr = HilRequest {
            id: Uuid::new_v4(),
            agent_id: Uuid::new_v4(),
            prompt: "test prompt".to_string(),
            context: vec!["ctx1".to_string()],
        };
        let s = serde_json::to_string(&hr).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["prompt"], "test prompt");
        assert_eq!(v["context"][0], "ctx1");
    }

    #[test]
    fn notification_kind_serde() {
        let nk = NotificationKind::Hil;
        let s = serde_json::to_string(&nk).unwrap();
        let d: NotificationKind = serde_json::from_str(&s).unwrap();
        assert_eq!(d, NotificationKind::Hil);
    }
}
