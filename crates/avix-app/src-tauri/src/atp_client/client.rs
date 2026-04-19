use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::instrument;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentCommand {
    Spawn { name: String, goal: String },
    Kill { pid: u32 },
    Pause { pid: u32 },
    Resume { pid: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentCommandError {
    NotConnected,
    InvalidResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AtpEvent {
    StatusChanged {
        pid: u32,
        status: String,
    },
    ToolChanged {
        pid: u32,
        tool: String,
        action: String,
    },
    HilRequest {
        pid: u32,
        hil_id: String,
        description: String,
    },
}

impl AtpEvent {
    #[instrument]
    pub fn from_json(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}

impl AgentCommand {
    #[instrument]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_spawn_command_serializes() {
        let cmd = AgentCommand::Spawn {
            name: "test".into(),
            goal: "do stuff".into(),
        };
        let json = cmd.to_json();
        assert_eq!(json["type"], "spawn");
        assert_eq!(json["name"], "test");
    }

    #[test]
    fn test_kill_command_serializes() {
        let cmd = AgentCommand::Kill { pid: 42 };
        let json = cmd.to_json();
        assert_eq!(json["type"], "kill");
        assert_eq!(json["pid"], 42);
    }

    #[test]
    fn test_pause_command_serializes() {
        let cmd = AgentCommand::Pause { pid: 42 };
        let json = cmd.to_json();
        assert_eq!(json["type"], "pause");
    }

    #[test]
    fn test_resume_command_serializes() {
        let cmd = AgentCommand::Resume { pid: 42 };
        let json = cmd.to_json();
        assert_eq!(json["type"], "resume");
    }

    #[test]
    fn test_status_changed_event_parses() {
        let v = json!({ "event": "status_changed", "pid": 42, "status": "running" });
        let event = AtpEvent::from_json(&v);
        assert!(matches!(
            event,
            Some(AtpEvent::StatusChanged { pid: 42, .. })
        ));
    }

    #[test]
    fn test_tool_changed_event_parses() {
        let v = json!({
            "event": "tool_changed",
            "pid": 42,
            "tool": "fs/read",
            "action": "added"
        });
        let event = AtpEvent::from_json(&v);
        assert!(matches!(event, Some(AtpEvent::ToolChanged { .. })));
    }

    #[test]
    fn test_hil_request_event_parses() {
        let v = json!({
            "event": "hil_request",
            "pid": 42,
            "hil_id": "abc123",
            "description": "approve tool call"
        });
        let event = AtpEvent::from_json(&v);
        assert!(matches!(event, Some(AtpEvent::HilRequest { .. })));
    }

    #[test]
    fn test_unknown_event_returns_none() {
        let v = json!({ "event": "unknown_xyz", "pid": 1 });
        let event = AtpEvent::from_json(&v);
        assert!(event.is_none());
    }

    #[test]
    fn test_command_roundtrip() {
        let cmd = AgentCommand::Spawn {
            name: "test".into(),
            goal: "goal".into(),
        };
        let json = cmd.to_json();
        let restored: AgentCommand = serde_json::from_value(json).unwrap();
        assert_eq!(cmd, restored);
    }

    #[test]
    fn test_event_roundtrip() {
        let event = AtpEvent::StatusChanged {
            pid: 42,
            status: "running".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        let restored: AtpEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event, restored);
    }

    #[test]
    fn test_spawn_with_empty_name() {
        let cmd = AgentCommand::Spawn {
            name: "".into(),
            goal: "goal".into(),
        };
        let json = cmd.to_json();
        assert_eq!(json["name"], "");
    }

    #[test]
    fn test_status_changed_status_field() {
        let v = json!({ "event": "status_changed", "pid": 10, "status": "paused" });
        if let Some(AtpEvent::StatusChanged { status, .. }) = AtpEvent::from_json(&v) {
            assert_eq!(status, "paused");
        } else {
            panic!("Expected StatusChanged");
        }
    }

    #[test]
    fn test_hil_description_preserved() {
        let v = json!({
            "event": "hil_request",
            "pid": 1,
            "hil_id": "hil-1",
            "description": "approve this action"
        });
        if let Some(AtpEvent::HilRequest { description, .. }) = AtpEvent::from_json(&v) {
            assert_eq!(description, "approve this action");
        } else {
            panic!("Expected HilRequest");
        }
    }

    #[test]
    fn test_tool_action_preserved() {
        let v = json!({
            "event": "tool_changed",
            "pid": 5,
            "tool": "cap/request-tool",
            "action": "removed"
        });
        if let Some(AtpEvent::ToolChanged { action, .. }) = AtpEvent::from_json(&v) {
            assert_eq!(action, "removed");
        } else {
            panic!("Expected ToolChanged");
        }
    }

    #[test]
    fn test_spawn_goal_preserved() {
        let cmd = AgentCommand::Spawn {
            name: "agent".into(),
            goal: "analyze data".into(),
        };
        let json = cmd.to_json();
        assert_eq!(json["goal"], "analyze data");
    }
}
