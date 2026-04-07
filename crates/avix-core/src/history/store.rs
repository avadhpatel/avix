use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::record::{MessageRecord, PartRecord, PartType};
use crate::error::AvixError;

const MESSAGE_TABLE: TableDefinition<&str, &str> = TableDefinition::new("messages");
const PART_TABLE: TableDefinition<&str, &str> = TableDefinition::new("parts");

/// Persistent store for `MessageRecord` and `PartRecord`.
///
/// Backed by redb at `<avix_root>/users/<username>/.avix_data/history.redb`.
/// Provides create/get/list for both entity types plus query helpers.
pub struct HistoryStore {
    db: Arc<Mutex<Database>>,
}

impl HistoryStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            write_txn
                .open_table(MESSAGE_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            write_txn
                .open_table(PART_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
    }

    // ── Message operations ────────────────────────────────────────────────────

    pub async fn create_message(&self, msg: &MessageRecord) -> Result<(), AvixError> {
        let json = serde_json::to_string(msg).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let key = msg.id.to_string();
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(MESSAGE_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(key.as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    pub async fn get_message(&self, id: &Uuid) -> Result<Option<MessageRecord>, AvixError> {
        let key = id.to_string();
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(MESSAGE_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table
            .get(key.as_str())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            Some(v) => Ok(Some(
                serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    /// List all messages for `session_id`, ordered by `sequence`.
    pub async fn list_messages(&self, session_id: &Uuid) -> Result<Vec<MessageRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(MESSAGE_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut messages = Vec::new();
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
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

    // ── Part operations ───────────────────────────────────────────────────────

    pub async fn create_part(&self, part: &PartRecord) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(part).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let key = part.id.to_string();
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(PART_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(key.as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    pub async fn get_part(&self, id: &Uuid) -> Result<Option<PartRecord>, AvixError> {
        let key = id.to_string();
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(PART_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table
            .get(key.as_str())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            Some(v) => Ok(Some(
                serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    /// List all parts for `message_id`, ordered by `part_index`.
    pub async fn list_parts(&self, message_id: &Uuid) -> Result<Vec<PartRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(PART_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut parts = Vec::new();
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
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

    // ── Query helpers ─────────────────────────────────────────────────────────

    /// Get all tool-call parts across all messages in a session.
    pub async fn get_session_tool_calls(
        &self,
        session_id: &Uuid,
    ) -> Result<Vec<PartRecord>, AvixError> {
        self.get_session_parts_by_type(session_id, &PartType::ToolCall)
            .await
    }

    /// Get all file-diff parts across all messages in a session.
    pub async fn get_session_file_diffs(
        &self,
        session_id: &Uuid,
    ) -> Result<Vec<PartRecord>, AvixError> {
        self.get_session_parts_by_type(session_id, &PartType::FileDiff)
            .await
    }

    async fn get_session_parts_by_type(
        &self,
        session_id: &Uuid,
        part_type: &PartType,
    ) -> Result<Vec<PartRecord>, AvixError> {
        let messages = self.list_messages(session_id).await?;
        let mut result = Vec::new();
        for msg in messages {
            let parts = self.list_parts(&msg.id).await?;
            for part in parts {
                if &part.part_type == part_type {
                    result.push(part);
                }
            }
        }
        Ok(result)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::record::{MessageRecord, PartRecord, PartType, Role};
    use chrono::Utc;
    use tempfile::tempdir;

    async fn open_store() -> HistoryStore {
        let dir = tempdir().unwrap();
        HistoryStore::open(dir.path().join("history.redb"))
            .await
            .unwrap()
    }

    fn make_msg(session_id: Uuid, sequence: u64) -> MessageRecord {
        MessageRecord {
            id: Uuid::new_v4(),
            session_id,
            sequence,
            role: Role::User,
            timestamp: Utc::now(),
            content: format!("message {sequence}"),
            tokens: None,
        }
    }

    // T-HIST-01
    #[tokio::test]
    async fn create_and_get_message_roundtrip() {
        let store = open_store().await;
        let session_id = Uuid::new_v4();
        let msg = make_msg(session_id, 1);
        let msg_id = msg.id;
        store.create_message(&msg).await.unwrap();
        let loaded = store.get_message(&msg_id).await.unwrap().unwrap();
        assert_eq!(loaded.content, "message 1");
        assert_eq!(loaded.session_id, session_id);
    }

    // T-HIST-02
    #[tokio::test]
    async fn list_messages_filters_by_session() {
        let store = open_store().await;
        let session1 = Uuid::new_v4();
        let session2 = Uuid::new_v4();

        store.create_message(&make_msg(session1, 1)).await.unwrap();
        store.create_message(&make_msg(session1, 2)).await.unwrap();
        store.create_message(&make_msg(session2, 1)).await.unwrap();

        let msgs = store.list_messages(&session1).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().all(|m| m.session_id == session1));
    }

    // T-HIST-03
    #[tokio::test]
    async fn list_messages_ordered_by_sequence() {
        let store = open_store().await;
        let session_id = Uuid::new_v4();

        store
            .create_message(&make_msg(session_id, 3))
            .await
            .unwrap();
        store
            .create_message(&make_msg(session_id, 1))
            .await
            .unwrap();
        store
            .create_message(&make_msg(session_id, 2))
            .await
            .unwrap();

        let msgs = store.list_messages(&session_id).await.unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].sequence, 1);
        assert_eq!(msgs[1].sequence, 2);
        assert_eq!(msgs[2].sequence, 3);
    }

    // T-HIST-04
    #[tokio::test]
    async fn create_and_list_parts() {
        let store = open_store().await;
        let msg_id = Uuid::new_v4();

        let part1 = PartRecord::text(msg_id, 0, "Hello");
        let part2 = PartRecord::tool_call(
            msg_id,
            1,
            "call-1",
            "fs/read",
            serde_json::json!({"path": "/foo"}),
            None,
        );

        store.create_part(&part1).await.unwrap();
        store.create_part(&part2).await.unwrap();

        let parts = store.list_parts(&msg_id).await.unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].part_type, PartType::Text);
        assert_eq!(parts[1].part_type, PartType::ToolCall);
    }

    // T-HIST-05
    #[tokio::test]
    async fn list_parts_ordered_by_part_index() {
        let store = open_store().await;
        let msg_id = Uuid::new_v4();

        store
            .create_part(&PartRecord::text(msg_id, 2, "third"))
            .await
            .unwrap();
        store
            .create_part(&PartRecord::text(msg_id, 0, "first"))
            .await
            .unwrap();
        store
            .create_part(&PartRecord::text(msg_id, 1, "second"))
            .await
            .unwrap();

        let parts = store.list_parts(&msg_id).await.unwrap();
        assert_eq!(parts[0].data["content"], "first");
        assert_eq!(parts[1].data["content"], "second");
        assert_eq!(parts[2].data["content"], "third");
    }

    // T-HIST-06
    #[tokio::test]
    async fn get_session_tool_calls_returns_all() {
        let store = open_store().await;
        let session_id = Uuid::new_v4();

        // msg1 has 1 tool call + 1 text
        let msg1 = make_msg(session_id, 1);
        store.create_message(&msg1).await.unwrap();
        store
            .create_part(&PartRecord::tool_call(
                msg1.id,
                0,
                "c1",
                "fs/read",
                serde_json::json!({}),
                None,
            ))
            .await
            .unwrap();
        store
            .create_part(&PartRecord::text(msg1.id, 1, "content"))
            .await
            .unwrap();

        // msg2 has 1 tool call
        let msg2 = make_msg(session_id, 2);
        store.create_message(&msg2).await.unwrap();
        store
            .create_part(&PartRecord::tool_call(
                msg2.id,
                0,
                "c2",
                "fs/write",
                serde_json::json!({}),
                None,
            ))
            .await
            .unwrap();

        let tool_calls = store.get_session_tool_calls(&session_id).await.unwrap();
        assert_eq!(tool_calls.len(), 2);
        assert!(tool_calls.iter().all(|p| p.part_type == PartType::ToolCall));
    }

    // T-HIST-07
    #[tokio::test]
    async fn get_session_file_diffs_returns_only_file_diff_parts() {
        let store = open_store().await;
        let session_id = Uuid::new_v4();

        let msg = make_msg(session_id, 1);
        store.create_message(&msg).await.unwrap();
        store
            .create_part(&PartRecord::file_diff(
                msg.id,
                0,
                "/foo.rs",
                Some("+fn main(){}"),
                None,
            ))
            .await
            .unwrap();
        store
            .create_part(&PartRecord::tool_call(
                msg.id,
                1,
                "c1",
                "fs/read",
                serde_json::json!({}),
                None,
            ))
            .await
            .unwrap();

        let diffs = store.get_session_file_diffs(&session_id).await.unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].part_type, PartType::FileDiff);
    }

    // T-HIST-08
    #[tokio::test]
    async fn get_message_returns_none_for_unknown_id() {
        let store = open_store().await;
        let result = store.get_message(&Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    // T-HIST-09
    #[tokio::test]
    async fn get_part_returns_none_for_unknown_id() {
        let store = open_store().await;
        let result = store.get_part(&Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    // T-HIST-10
    #[tokio::test]
    async fn create_and_get_part_roundtrip() {
        let store = open_store().await;
        let msg_id = Uuid::new_v4();
        let part = PartRecord::thought(msg_id, 0, "reasoning here");
        let part_id = part.id;
        store.create_part(&part).await.unwrap();
        let loaded = store.get_part(&part_id).await.unwrap().unwrap();
        assert_eq!(loaded.part_type, PartType::Thought);
        assert_eq!(loaded.data["reasoning"], "reasoning here");
    }
}
