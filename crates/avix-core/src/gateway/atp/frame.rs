use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{AtpError, AtpFrameError};
use super::types::{AtpDomain, AtpEventKind};

/// A command sent from client → gateway (§5.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpCmd {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub id: String,
    pub token: String,
    pub domain: AtpDomain,
    pub op: String,
    #[serde(default)]
    pub body: Value,
}

/// A reply sent from gateway → client, correlated by `id` (§5.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpReply {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AtpError>,
}

impl AtpReply {
    pub fn ok(id: impl Into<String>, body: Value) -> Self {
        Self {
            msg_type: "reply".into(),
            id: id.into(),
            ok: true,
            body: Some(body),
            error: None,
        }
    }

    pub fn err(id: impl Into<String>, error: AtpError) -> Self {
        Self {
            msg_type: "reply".into(),
            id: id.into(),
            ok: false,
            body: None,
            error: Some(error),
        }
    }
}

/// A server-push event sent from gateway → client (§5.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpEvent {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub event: AtpEventKind,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub ts: DateTime<Utc>,
    pub body: Value,
}

impl AtpEvent {
    pub fn new(event: AtpEventKind, session_id: impl Into<String>, body: Value) -> Self {
        Self {
            msg_type: "event".into(),
            event,
            session_id: session_id.into(),
            ts: Utc::now(),
            body,
        }
    }
}

/// A subscription request sent from client → gateway (§5.4).
/// `id` and `token` are optional — the WS connection is already authenticated at upgrade time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtpSubscribe {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub token: String,
    /// Event names to subscribe to, or `["*"]` for all permitted events.
    pub events: Vec<String>,
}

/// Top-level inbound frame. Outbound frames (`AtpReply`, `AtpEvent`) are used directly.
#[derive(Debug, Clone)]
pub enum AtpFrame {
    Cmd(AtpCmd),
    Subscribe(AtpSubscribe),
}

impl AtpFrame {
    /// Parse a raw JSON text frame into an `AtpFrame`.
    pub fn parse(raw: &str) -> Result<Self, AtpFrameError> {
        let v: Value =
            serde_json::from_str(raw).map_err(|e| AtpFrameError::Parse(e.to_string()))?;

        let msg_type = v["type"]
            .as_str()
            .ok_or_else(|| AtpFrameError::Parse("missing 'type' field".into()))?;

        match msg_type {
            "cmd" => {
                let cmd: AtpCmd =
                    serde_json::from_value(v).map_err(|e| AtpFrameError::Parse(e.to_string()))?;
                Ok(AtpFrame::Cmd(cmd))
            }
            "subscribe" => {
                let sub: AtpSubscribe =
                    serde_json::from_value(v).map_err(|e| AtpFrameError::Parse(e.to_string()))?;
                Ok(AtpFrame::Subscribe(sub))
            }
            other => Err(AtpFrameError::UnknownType(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::error::AtpErrorCode;
    use serde_json::json;

    #[test]
    fn parse_cmd_frame() {
        let raw = r#"{
            "type": "cmd",
            "id": "c-001",
            "token": "tok",
            "domain": "proc",
            "op": "spawn",
            "body": { "agent": "researcher" }
        }"#;
        let frame = AtpFrame::parse(raw).unwrap();
        match frame {
            AtpFrame::Cmd(cmd) => {
                assert_eq!(cmd.id, "c-001");
                assert_eq!(cmd.domain, AtpDomain::Proc);
                assert_eq!(cmd.op, "spawn");
                assert_eq!(cmd.body["agent"], "researcher");
            }
            _ => panic!("expected Cmd"),
        }
    }

    #[test]
    fn parse_cmd_frame_without_body_defaults_to_null() {
        let raw =
            r#"{ "type": "cmd", "id": "c-002", "token": "t", "domain": "proc", "op": "list" }"#;
        let frame = AtpFrame::parse(raw).unwrap();
        assert!(matches!(frame, AtpFrame::Cmd(_)));
    }

    #[test]
    fn parse_subscribe_frame() {
        let raw = r#"{
            "type": "subscribe",
            "id": "sub-001",
            "token": "tok",
            "events": ["agent.output", "agent.status"]
        }"#;
        let frame = AtpFrame::parse(raw).unwrap();
        match frame {
            AtpFrame::Subscribe(sub) => {
                assert_eq!(sub.id, "sub-001");
                assert_eq!(sub.events, vec!["agent.output", "agent.status"]);
            }
            _ => panic!("expected Subscribe"),
        }
    }

    #[test]
    fn parse_unknown_type_returns_error() {
        let raw = r#"{ "type": "unknown" }"#;
        assert!(matches!(
            AtpFrame::parse(raw),
            Err(AtpFrameError::UnknownType(_))
        ));
    }

    #[test]
    fn parse_missing_type_returns_error() {
        let raw = r#"{ "id": "x" }"#;
        assert!(matches!(AtpFrame::parse(raw), Err(AtpFrameError::Parse(_))));
    }

    #[test]
    fn parse_malformed_json_returns_error() {
        let raw = r#"{ not valid json "#;
        assert!(matches!(AtpFrame::parse(raw), Err(AtpFrameError::Parse(_))));
    }

    #[test]
    fn reply_ok_serializes_correctly() {
        let reply = AtpReply::ok("c-001", json!({ "pid": 42 }));
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains("\"type\":\"reply\""));
        assert!(s.contains("\"ok\":true"));
        assert!(s.contains("\"pid\":42"));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn reply_err_serializes_correctly() {
        let err = AtpError::new(AtpErrorCode::Eperm, "not allowed");
        let reply = AtpReply::err("c-002", err);
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains("\"type\":\"reply\""));
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("\"EPERM\""));
        assert!(s.contains("not allowed"));
        assert!(!s.contains("\"body\""));
    }

    #[test]
    fn reply_ok_omits_error_field() {
        let reply = AtpReply::ok("c-001", json!({}));
        let s = serde_json::to_string(&reply).unwrap();
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn reply_err_omits_body_field() {
        let reply = AtpReply::err("c-001", AtpError::new(AtpErrorCode::Eauth, "bad token"));
        let s = serde_json::to_string(&reply).unwrap();
        assert!(!s.contains("\"body\""));
    }

    #[test]
    fn reply_id_is_preserved() {
        let reply = AtpReply::ok("my-request-id-42", json!({}));
        assert_eq!(reply.id, "my-request-id-42");
    }

    #[test]
    fn event_has_correct_type_and_fields() {
        let ev = AtpEvent::new(
            AtpEventKind::AgentOutput,
            "sess-001",
            json!({ "pid": 57, "text": "hello" }),
        );
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"type\":\"event\""));
        assert!(s.contains("\"agent.output\""));
        assert!(s.contains("\"sessionId\":\"sess-001\""));
        assert!(s.contains("\"ts\""));
        assert!(s.contains("\"pid\":57"));
    }

    #[test]
    fn event_session_id_serializes_as_camel_case() {
        let ev = AtpEvent::new(AtpEventKind::SessionReady, "sess-abc", json!({}));
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"sessionId\""));
        assert!(!s.contains("\"session_id\""));
    }

    #[test]
    fn all_domains_parse_from_cmd_frame() {
        for (domain_str, expected) in [
            ("auth", AtpDomain::Auth),
            ("proc", AtpDomain::Proc),
            ("signal", AtpDomain::Signal),
            ("fs", AtpDomain::Fs),
            ("snap", AtpDomain::Snap),
            ("cron", AtpDomain::Cron),
            ("users", AtpDomain::Users),
            ("crews", AtpDomain::Crews),
            ("cap", AtpDomain::Cap),
            ("sys", AtpDomain::Sys),
            ("pipe", AtpDomain::Pipe),
        ] {
            let raw = format!(
                r#"{{"type":"cmd","id":"x","token":"t","domain":"{domain_str}","op":"list"}}"#
            );
            let frame = AtpFrame::parse(&raw).unwrap();
            match frame {
                AtpFrame::Cmd(cmd) => assert_eq!(cmd.domain, expected),
                _ => panic!("expected Cmd for domain {domain_str}"),
            }
        }
    }
}
