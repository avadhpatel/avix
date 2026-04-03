use serde::{Deserialize, Serialize};

/// Role in a conversation turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

/// A single turn in the conversation history.
///
/// This struct supports both flat format (v1) and structured format (v2).
/// Parsers should handle both formats gracefully.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEntry {
    /// Role of the message sender.
    pub role: Role,
    /// Human-readable content (always present for backward compatibility).
    pub content: String,
    /// Structured tool calls made by the assistant (v2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallEntry>,
    /// File changes that resulted from this turn (v2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<FileDiffEntry>,
    /// Reasoning/thought trace (v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<String>,
}

impl ConversationEntry {
    /// Parse a JSON line that may be in v1 (flat) or v2 (structured) format.
    pub fn from_json_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }

    /// Create a simple entry from role and content (v1 compatibility).
    pub fn from_role_content(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            files_changed: Vec::new(),
            thought: None,
        }
    }
}

/// A single tool call with arguments and result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEntry {
    /// Unique ID for this tool call.
    pub id: String,
    /// Name of the tool (e.g., "fs/read").
    pub name: String,
    /// Arguments passed to the tool.
    pub args: serde_json::Value,
    /// Result returned by the tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// A file change with diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffEntry {
    /// Absolute VFS path to the file.
    pub path: String,
    /// Diff content (unified format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// Full new content (for new files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // T-CONV-01
    #[test]
    fn flat_entry_parses_correctly() {
        let line = r#"{"role": "user", "content": "Hello"}"#;
        let entry = ConversationEntry::from_json_line(line).unwrap();
        assert_eq!(entry.role, Role::User);
        assert_eq!(entry.content, "Hello");
        assert!(entry.tool_calls.is_empty());
    }

    // T-CONV-02
    #[test]
    fn structured_entry_parses_tool_calls() {
        let line = r#"{
            "role": "assistant",
            "content": "Reading file",
            "toolCalls": [{"id": "call-1", "name": "fs/read", "args": {"path": "/foo"}, "result": {"content": "bar"}}]
        }"#;
        let entry = ConversationEntry::from_json_line(line).unwrap();
        assert_eq!(entry.role, Role::Assistant);
        assert_eq!(entry.tool_calls.len(), 1);
        assert_eq!(entry.tool_calls[0].name, "fs/read");
    }

    // T-CONV-03
    #[test]
    fn structured_entry_parses_files_changed() {
        let line = r#"{
            "role": "assistant",
            "content": "Wrote file",
            "filesChanged": [{"path": "/foo.txt", "diff": "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new"}]
        }"#;
        let entry = ConversationEntry::from_json_line(line).unwrap();
        assert_eq!(entry.files_changed.len(), 1);
        assert!(entry.files_changed[0].diff.is_some());
    }

    // T-CONV-04
    #[test]
    fn structured_entry_parses_thought() {
        let line = r#"{"role": "assistant", "content": "Done", "thought": "I should check the docs first"}"#;
        let entry = ConversationEntry::from_json_line(line).unwrap();
        assert_eq!(
            entry.thought,
            Some("I should check the docs first".to_string())
        );
    }

    // T-CONV-05
    #[test]
    fn roundtrip_structured_entry() {
        let entry = ConversationEntry {
            role: Role::Assistant,
            content: "Hello".to_string(),
            tool_calls: vec![ToolCallEntry {
                id: "call-1".to_string(),
                name: "fs/read".to_string(),
                args: serde_json::json!({"path": "/foo"}),
                result: Some(serde_json::json!({"content": "bar"})),
            }],
            files_changed: vec![],
            thought: Some("checking".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ConversationEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_calls[0].name, "fs/read");
    }

    // T-CONV-06
    #[test]
    fn from_role_content_creates_simple_entry() {
        let entry = ConversationEntry::from_role_content(Role::User, "Hello");
        assert_eq!(entry.role, Role::User);
        assert_eq!(entry.content, "Hello");
        assert!(entry.tool_calls.is_empty());
    }

    // T-CONV-07
    #[test]
    fn tool_call_entry_roundtrip() {
        let tc = ToolCallEntry {
            id: "call-123".to_string(),
            name: "fs/write".to_string(),
            args: serde_json::json!({"path": "/test.txt", "content": "hello"}),
            result: Some(serde_json::json!({"success": true})),
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: ToolCallEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "call-123");
        assert_eq!(parsed.name, "fs/write");
        assert_eq!(parsed.args["path"], "/test.txt");
    }
}
