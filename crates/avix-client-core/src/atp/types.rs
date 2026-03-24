use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type", skip_deserializing, default)]
    pub frame_type: String, // always "event"
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
    Raw(Value), // fallback for unrecognised / future kinds
}

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
pub enum AgentStatus {
    Running,
    Paused,
    Stopped,
    Crashed,
}

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
