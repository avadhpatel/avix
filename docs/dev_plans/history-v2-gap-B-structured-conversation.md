# history-v2-gap-B: Structured Conversation Format

## Specification Reference

- **Spec**: `docs/specs/agent-history-persistence-v2.md`
- **Phase**: v2.0 (Short-term)
- **Goal**: Replace flat JSONL with structured entries supporting tool_calls, files_changed, thought

## What This Builds

Extends the conversation JSONL format to support structured entries with typed fields. Maintains backward compatibility with existing flat entries.

## Storage Location

All user-specific data (including redb) is stored in:
```
<AVIX_ROOT>/users/<username>/.avix_data/
├── invocations.redb    ← primary store for InvocationRecord
├── sessions.redb       ← (future) SessionStore
└── history.redb       ← (future) HistoryStore for MessageRecord/PartRecord
```

Conversation JSONL is written via LocalProvider to:
```
<AVIX_ROOT>/users/<username>/agents/<agent_name>/invocations/<id>/
├── <id>.yaml           ← summary
└── conversation.jsonl  ← conversation (v2 structured format)
```

## Implementation Guidance

### 1. Define new conversation entry types

Location: New file `crates/avix-core/src/invocation/conversation.rs` (or add to existing module)

```rust
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
```

### 2. Backward-compatible parser

```rust
impl ConversationEntry {
    /// Parse a JSON line that may be in v1 (flat) or v2 (structured) format.
    pub fn from_json_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
        // v1 had: {"role": "user", "content": "..."}
        // v2 has additional fields
    }
}
```

The serde approach naturally handles backward compatibility — missing fields default to empty/None.

### 3. Update `InvocationStore::write_conversation`

Location: `crates/avix-core/src/invocation/store.rs`

```rust
pub async fn write_conversation(
    &self,
    id: &str,
    username: &str,
    agent_name: &str,
    messages: &[ConversationEntry],  // Changed from [(String, String)]
) -> Result<(), AvixError>
```

- Serialize each entry as JSON (one line per entry)
- Supports the new structured format
- Old flat format still works if passed as messages with only role/content

### 4. Update RuntimeExecutor to capture structured data

Location: `crates/avix-core/src/executor/runtime_executor.rs`

When building conversation history for persistence:

```rust
struct ConversationEntryBuilder {
    role: Role,
    content: String,
    tool_calls: Vec<ToolCallEntry>,
    files_changed: Vec<FileDiffEntry>,
    thought: Option<String>,
}

// After each tool call completes:
tool_calls.push(ToolCallEntry {
    id: tool_call_id,
    name: tool_name,
    args: serde_json::to_value(&args).unwrap(),
    result: Some(serde_json::to_value(&result).unwrap()),
});

// After each LLM turn:
if let Some(reasoning) = llm_response.thinking {
    thought = Some(reasoning);
}

// When build_conversation_for_save() is called:
entries.push(ConversationEntry {
    role: role_from_message(&msg),
    content: msg.content.clone(),
    tool_calls: std::mem::take(&mut self.tool_calls_buffer),
    files_changed: std::mem::take(&mut self.files_changed_buffer),
    thought: std::mem::take(&mut self.thought_buffer),
});
```

### 5. Add helper to extract structured data during runtime

In RuntimeExecutor, maintain temporary buffers that get populated during tool execution:

```rust
// In RuntimeExecutor struct
tool_call_buffer: Vec<ToolCallEntry>,
files_changed_buffer: Vec<FileDiffEntry>,
thought_buffer: Option<String>,
```

After each tool call, push to buffer. After LLM response, capture thought. When writing conversation (interim or final), flush buffers into entries.

### 6. Migration path

- Existing `conversation.jsonl` files with flat format remain readable
- New writes use structured format
- Parser handles both transparently

## TDD Tests

```rust
// In crates/avix-core/src/invocation/conversation.rs tests

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
        "tool_calls": [{"id": "call-1", "name": "fs/read", "args": {"path": "/foo"}, "result": {"content": "bar"}}]
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
        "files_changed": [{"path": "/foo.txt", "diff": "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new"}]
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
    assert_eq!(entry.thought, Some("I should check the docs first".to_string()));
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
```

## Success Criteria

- [ ] `ConversationEntry` supports flat (v1) and structured (v2) formats
- [ ] Backward-compatible: existing JSONL files parse correctly
- [ ] Tool calls captured with id, name, args, result
- [ ] File diffs captured with path and diff content
- [ ] Thought/reasoning captured from LLM response
- [ ] `write_conversation` accepts structured entries
- [ ] All tests pass: `cargo test --workspace`