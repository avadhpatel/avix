# history-v2-gap-D: Hierarchical Sessions (MessageRecord + PartRecord)

## Specification Reference

- **Spec**: `docs/specs/agent-history-persistence-v2.md`
- **Phase**: v2.1 (Medium-term)
- **Goal**: Adopt opencode's mental model with Session → Message → Part hierarchy

## What This Builds

Introduces `MessageRecord` and `PartRecord` entities alongside existing `SessionRecord`. Enables structured, queryable conversation history with typed parts (text, tool_call, file_diff, thought, etc.).

## Storage Location

All user-specific data (including redb) is stored in:
```
<AVIX_ROOT>/users/<username>/.avix_data/
├── invocations.redb    ← InvocationRecord
├── sessions.redb       ← SessionRecord (existing)
└── history.redb       ← NEW: MessageRecord + PartRecord
```

FS mirror continues at:
```
<AVIX_ROOT>/users/<username>/agents/<agent_name>/invocations/<id>/
├── <id>.yaml           ← summary
└── conversation.jsonl  ← conversation
```

## Implementation Guidance

### 1. New data structures

Location: New file `crates/avix-core/src/history/record.rs` (or extend existing session/record.rs)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── MessageRecord ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

/// A single message in a session's conversation history.
/// Corresponds to one LLM turn or tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: Uuid,
    pub session_id: Uuid,
    pub sequence: u64,              // Ordering within session
    pub role: Role,
    pub timestamp: DateTime<Utc>,
    pub content: String,            // Human-readable fallback
    pub tokens: Option<u64>,        // Token count for this message
}

// ── PartRecord ──────────────────────────────────────────────────────────────

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

/// A typed part of a message.
/// Enables fine-grained querying and structured storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartRecord {
    pub id: Uuid,
    pub message_id: Uuid,
    pub part_index: u32,            // Ordering within message
    pub part_type: PartType,
    pub data: serde_json::Value,    // Typed payload
}

// Helper constructors for common part types
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
```

### 2. New HistoryStore (or extend InvocationStore)

Location: `crates/avix-core/src/history/store.rs` (new module)

```rust
use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, TableDefinition};
use tokio::sync::Mutex;

use super::record::{MessageRecord, PartRecord};
use crate::error::AvixError;

const MESSAGE_TABLE: TableDefinition<&str, &str> = TableDefinition::new("messages");
const PART_TABLE: TableDefinition<&str, &str> = TableDefinition::new("parts");

pub struct HistoryStore {
    db: Arc<Mutex<Database>>,
}

impl HistoryStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        
        // Ensure tables exist
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            write_txn.open_table(MESSAGE_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            write_txn.open_table(PART_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        
        Ok(Self { db: Arc::new(Mutex::new(db)) })
    }

    // ── Message operations ─────────────────────────────────────────────────

    pub async fn create_message(&self, msg: &MessageRecord) -> Result<(), AvixError> {
        let json = serde_json::to_string(msg).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn.open_table(MESSAGE_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table.insert(msg.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    pub async fn get_message(&self, id: &Uuid) -> Result<Option<MessageRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn.open_table(MESSAGE_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table.get(id.to_string().as_str()).map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            Some(v) => Ok(Some(serde_json::from_str(v.value()).map_err(|e| AvixError::ConfigParse(e.to_string()))?)),
            None => Ok(None),
        }
    }

    pub async fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn.open_table(MESSAGE_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut messages = Vec::new();
        for item in table.iter().map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let msg: MessageRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            if msg.session_id == *session_id {
                messages.push(msg);
            }
        }
        messages.sort_by_key(|m| m.sequence);
        Ok(messages)
    }

    // ── Part operations ─────────────────────────────────────────────────────

    pub async fn create_part(&self, part: &PartRecord) -> Result<(), AvixError> {
        let json = serde_json::to_string(part).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn.open_table(PART_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table.insert(part.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    pub async fn list_parts(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn.open_table(PART_TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut parts = Vec::new();
        for item in table.iter().map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let part: PartRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            if part.message_id == *message_id {
                parts.push(part);
            }
        }
        parts.sort_by_key(|p| p.part_index);
        Ok(parts)
    }
}
```

### 3. Link existing Invocations to new Message/Part model

The existing `InvocationRecord` already has `session_id`. We need to populate the new tables from existing invocations:

```rust
/// Migration: Convert existing invocations to messages and parts.
pub async fn migrate_invocations(
    &self,
    invocation_store: &InvocationStore,
) -> Result<(), AvixError> {
    let invocations = invocation_store.list_all().await?;
    
    for inv in invocations {
        // Read conversation.jsonl
        // Parse each line → create MessageRecord + PartRecords
        // Insert into new tables
    }
    
    Ok(())
}
```

### 4. Extended ATP surface

Location: `crates/avix-core/src/kernel/proc.rs`

```rust
// New handlers
pub async fn handle_message_list(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError>
pub async fn handle_message_get(&self, message_id: &Uuid) -> Result<Option<MessageRecord>, AvixError>
pub async fn handle_part_list(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError>
pub async fn handle_part_get(&self, part_id: &Uuid) -> Result<Option<PartRecord>, AvixError>
```

```rust
// New ATP operations
"proc/message-list"   => "kernel/proc/message-list",
"proc/message-get"    => "kernel/proc/message-get", 
"proc/part-list"      => "kernel/proc/part-list",
"proc/part-get"       => "kernel/proc/part-get",
```

### 5. Query helpers for common patterns

```rust
impl HistoryStore {
    /// Get all tool calls in a session.
    pub async fn get_session_tool_calls(&self, session_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        let messages = self.list_messages(session_id).await?;
        let mut tool_calls = Vec::new();
        for msg in messages {
            let parts = self.list_parts(&msg.id).await?;
            for part in parts {
                if matches!(part.part_type, PartType::ToolCall) {
                    tool_calls.push(part);
                }
            }
        }
        Ok(tool_calls)
    }

    /// Get all file changes in a session.
    pub async fn get_session_file_diffs(&self, session_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        // Similar pattern to tool calls
    }
}
```

### 6. FS mirror updates (optional v2.1)

For human readability, also write to disk:

```
users/<username>/sessions/<session_id>/
├── session.yaml
├── messages/
│   ├── 0001-assistant.json
│   ├── 0002-tool.json
│   └── ...
└── parts/
    ├── <message_id>/
    │   ├── 0000-text.json
    │   ├── 0001-tool_call.json
    │   └── ...
```

This is optional — the redb store is the primary. FS mirror can be v2.2.

## TDD Tests

```rust
// In crates/avix-core/src/history/store.rs tests

// T-HIST-01
#[tokio::test]
async fn create_and_get_message_roundtrip() {
    let store = open_history_store().await;
    let msg = MessageRecord {
        id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        sequence: 1,
        role: Role::User,
        timestamp: Utc::now(),
        content: "Hello".to_string(),
        tokens: None,
    };
    store.create_message(&msg).await.unwrap();
    let loaded = store.get_message(&msg.id).await.unwrap().unwrap();
    assert_eq!(loaded.content, "Hello");
}

// T-HIST-02
#[tokio::test]
async fn list_messages_filters_by_session() {
    let store = open_history_store().await;
    let session1 = Uuid::new_v4();
    let session2 = Uuid::new_v4();
    
    // Create messages for session1
    store.create_message(&make_msg(session1, 1)).await;
    store.create_message(&make_msg(session1, 2)).await;
    // Create message for session2
    store.create_message(&make_msg(session2, 1)).await;
    
    let msgs = store.list_messages(&session1).await.unwrap();
    assert_eq!(msgs.len(), 2);
}

// T-HIST-03
#[tokio::test]
async fn create_and_list_parts() {
    let store = open_history_store().await;
    let msg_id = Uuid::new_v4();
    
    let part1 = PartRecord::text(msg_id, 0, "Hello");
    let part2 = PartRecord::tool_call(msg_id, 1, "call-1", "fs/read", serde_json::json!({"path": "/foo"}), None);
    
    store.create_part(&part1).await.unwrap();
    store.create_part(&part2).await.unwrap();
    
    let parts = store.list_parts(&msg_id).await.unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].part_type, PartType::Text);
    assert_eq!(parts[1].part_type, PartType::ToolCall);
}

// T-HIST-04
#[tokio::test]
async fn get_session_tool_calls_returns_all() {
    // Create session with multiple messages
    // Call get_session_tool_calls
    // Verify all tool_call parts returned
}
```

## Success Criteria

- [ ] `MessageRecord` and `PartRecord` types defined with all fields
- [ ] `HistoryStore` with create/get/list for messages and parts
- [ ] `PartType` enum supports: Text, ToolCall, FileDiff, CodeBlock, Thought
- [ ] Migration path: existing invocations → messages + parts
- [ ] ATP handlers: `message-list`, `message-get`, `part-list`, `part-get`
- [ ] Query helpers: `get_session_tool_calls`, `get_session_file_diffs`
- [ ] All tests pass: `cargo test --workspace`
- [ ] Backward compatible: existing ATP/CLI unchanged