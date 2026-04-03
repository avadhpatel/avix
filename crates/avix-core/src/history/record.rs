use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Role ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

// ── MessageRecord ─────────────────────────────────────────────────────────────

/// A single message in a session's conversation history.
/// Corresponds to one LLM turn or tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    /// Ordering within the session (monotonically increasing).
    pub sequence: u64,
    pub role: Role,
    pub timestamp: DateTime<Utc>,
    /// Human-readable fallback content.
    pub content: String,
    /// Token count for this message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}

// ── PartType ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartType {
    Text,
    ToolCall,
    FileDiff,
    CodeBlock,
    Thought,
    Image,
    Embedding,
    Summary,
}

// ── PartRecord ────────────────────────────────────────────────────────────────

/// A typed part of a message. Enables fine-grained querying and structured storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartRecord {
    pub id: Uuid,
    pub message_id: Uuid,
    /// Ordering within the message (zero-based).
    pub part_index: u32,
    pub part_type: PartType,
    /// Typed payload — schema depends on `part_type`.
    pub data: serde_json::Value,
}

impl PartRecord {
    pub fn text(message_id: Uuid, index: u32, content: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_id,
            part_index: index,
            part_type: PartType::Text,
            data: serde_json::json!({ "content": content }),
        }
    }

    pub fn tool_call(
        message_id: Uuid,
        index: u32,
        call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        result: Option<serde_json::Value>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_id,
            part_index: index,
            part_type: PartType::ToolCall,
            data: serde_json::json!({
                "call_id": call_id,
                "tool_name": tool_name,
                "args": args,
                "result": result,
            }),
        }
    }

    pub fn file_diff(
        message_id: Uuid,
        index: u32,
        path: &str,
        diff: Option<&str>,
        content: Option<&str>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_id,
            part_index: index,
            part_type: PartType::FileDiff,
            data: serde_json::json!({
                "path": path,
                "diff": diff,
                "content": content,
            }),
        }
    }

    pub fn thought(message_id: Uuid, index: u32, reasoning: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            message_id,
            part_index: index,
            part_type: PartType::Thought,
            data: serde_json::json!({ "reasoning": reasoning }),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // T-REC-01
    #[test]
    fn message_record_roundtrip_json() {
        let msg = MessageRecord {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            sequence: 1,
            role: Role::User,
            timestamp: Utc::now(),
            content: "Hello".to_string(),
            tokens: Some(10),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: MessageRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "Hello");
        assert_eq!(parsed.role, Role::User);
        assert_eq!(parsed.tokens, Some(10));
    }

    // T-REC-02
    #[test]
    fn part_text_constructor() {
        let msg_id = Uuid::new_v4();
        let part = PartRecord::text(msg_id, 0, "hello world");
        assert_eq!(part.part_type, PartType::Text);
        assert_eq!(part.data["content"], "hello world");
        assert_eq!(part.part_index, 0);
        assert_eq!(part.message_id, msg_id);
    }

    // T-REC-03
    #[test]
    fn part_tool_call_constructor() {
        let msg_id = Uuid::new_v4();
        let part = PartRecord::tool_call(
            msg_id,
            1,
            "call-1",
            "fs/read",
            serde_json::json!({"path": "/foo"}),
            Some(serde_json::json!({"content": "bar"})),
        );
        assert_eq!(part.part_type, PartType::ToolCall);
        assert_eq!(part.data["tool_name"], "fs/read");
        assert_eq!(part.data["args"]["path"], "/foo");
        assert_eq!(part.data["result"]["content"], "bar");
    }

    // T-REC-04
    #[test]
    fn part_file_diff_constructor() {
        let msg_id = Uuid::new_v4();
        let part = PartRecord::file_diff(msg_id, 0, "/src/main.rs", Some("+ fn main() {}"), None);
        assert_eq!(part.part_type, PartType::FileDiff);
        assert_eq!(part.data["path"], "/src/main.rs");
        assert!(part.data["diff"].is_string());
    }

    // T-REC-05
    #[test]
    fn part_thought_constructor() {
        let msg_id = Uuid::new_v4();
        let part = PartRecord::thought(msg_id, 0, "I should check the docs first");
        assert_eq!(part.part_type, PartType::Thought);
        assert_eq!(part.data["reasoning"], "I should check the docs first");
    }

    // T-REC-06
    #[test]
    fn part_record_roundtrip_json() {
        let msg_id = Uuid::new_v4();
        let part = PartRecord::tool_call(msg_id, 2, "c1", "fs/write", serde_json::json!({}), None);
        let json = serde_json::to_string(&part).unwrap();
        let parsed: PartRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.part_type, PartType::ToolCall);
        assert_eq!(parsed.part_index, 2);
    }

    // T-REC-07
    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        assert_eq!(serde_json::to_string(&Role::Tool).unwrap(), "\"tool\"");
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), "\"system\"");
    }

    // T-REC-08
    #[test]
    fn part_type_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&PartType::ToolCall).unwrap(),
            "\"toolcall\""
        );
        assert_eq!(
            serde_json::to_string(&PartType::FileDiff).unwrap(),
            "\"filediff\""
        );
        assert_eq!(
            serde_json::to_string(&PartType::Thought).unwrap(),
            "\"thought\""
        );
    }
}
