use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::warn;

use super::conversation::ConversationEntry;
use super::record::{InvocationRecord, InvocationStatus};
use crate::error::AvixError;
use crate::memfs::local_provider::LocalProvider;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("invocations");

/// Persistent store for agent invocation records.
///
/// Primary store: `redb` — fast keyed lookups for list/get operations.
/// Artefacts: `LocalProvider` — human-readable YAML summary + JSONL conversation
/// written to `<root>/users/<username>/agents/<agent>/invocations/`.
///
/// The `local` provider is optional. When absent, only redb is used (useful in tests).
pub struct InvocationStore {
    db: Arc<Mutex<Database>>,
    local: Option<Arc<LocalProvider>>,
}

impl InvocationStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        // Ensure the table exists.
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            local: None,
        })
    }

    /// Attach a `LocalProvider` rooted at `<avix_root>/users/` for disk artefacts.
    pub fn with_local(mut self, provider: LocalProvider) -> Self {
        self.local = Some(Arc::new(provider));
        self
    }

    // ── Write operations ──────────────────────────────────────────────────────

    /// Persist a new `InvocationRecord` (status: Running).
    pub async fn create(&self, record: &InvocationRecord) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(record.id.as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(record).await;
        Ok(())
    }

    /// Update a record's terminal fields after the agent exits.
    ///
    /// Idempotent: silently succeeds if `id` is not found.
    pub async fn finalize(
        &self,
        id: &str,
        status: InvocationStatus,
        ended_at: DateTime<Utc>,
        tokens_consumed: u64,
        tool_calls_total: u32,
        exit_reason: Option<String>,
    ) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()), // idempotent
        };
        record.status = status;
        record.ended_at = Some(ended_at);
        record.tokens_consumed = tokens_consumed;
        record.tool_calls_total = tool_calls_total;
        record.exit_reason = exit_reason;

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(id, json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(&record).await;
        Ok(())
    }

    /// Update only the status of a record (e.g., transition to Idle).
    pub async fn update_status(&self, id: &str, status: InvocationStatus) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        record.status = status;

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(id, json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(&record).await;
        Ok(())
    }

    /// Write interim snapshot of a running invocation.
    ///
    /// Unlike `finalize()`, this does NOT set ended_at or change status.
    /// It updates tokens/tool_calls and writes conversation to disk.
    ///
    /// Idempotent: silently succeeds if `id` is not found.
    pub async fn persist_interim(
        &self,
        id: &str,
        conversation: &[(String, String)],
        tokens_consumed: u64,
        tool_calls_total: u32,
    ) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        record.tokens_consumed = tokens_consumed;
        record.tool_calls_total = tool_calls_total;

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(id, json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(&record).await;

        if !conversation.is_empty() {
            self.write_conversation(id, &record.username, &record.agent_name, conversation)
                .await?;
        }

        Ok(())
    }

    /// Write interim snapshot with structured conversation entries.
    ///
    /// Unlike `finalize()`, this does NOT set ended_at or change status.
    /// It updates tokens/tool_calls and writes structured conversation to disk.
    ///
    /// Idempotent: silently succeeds if `id` is not found.
    pub async fn persist_interim_structured(
        &self,
        id: &str,
        entries: &[ConversationEntry],
        tokens_consumed: u64,
        tool_calls_total: u32,
    ) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        record.tokens_consumed = tokens_consumed;
        record.tool_calls_total = tool_calls_total;

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(id, json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(&record).await;

        if !entries.is_empty() {
            self.write_conversation_structured(id, &record.username, &record.agent_name, entries)
                .await?;
        }

        Ok(())
    }

    /// Append the full conversation history as a JSONL file.
    ///
    /// Each entry is `{"role": "<role>", "content": "<content>"}`.
    /// Written to `<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl`.
    pub async fn write_conversation(
        &self,
        id: &str,
        username: &str,
        agent_name: &str,
        messages: &[(String, String)],
    ) -> Result<(), AvixError> {
        let provider = match &self.local {
            Some(p) => p,
            None => return Ok(()),
        };
        let mut lines = String::new();
        for (role, content) in messages {
            let line = serde_json::json!({"role": role, "content": content});
            lines.push_str(
                &serde_json::to_string(&line).map_err(|e| AvixError::ConfigParse(e.to_string()))?,
            );
            lines.push('\n');
        }
        let rel = format!(
            "{}/agents/{}/invocations/{}/conversation.jsonl",
            username, agent_name, id
        );
        provider
            .write(&rel, lines.into_bytes())
            .await
            .map_err(|e| AvixError::Io(e.to_string()))
    }

    /// Write structured conversation entries as a JSONL file.
    ///
    /// Each entry is a `ConversationEntry` with optional tool_calls, files_changed, thought.
    /// Written to `<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl`.
    pub async fn write_conversation_structured(
        &self,
        id: &str,
        username: &str,
        agent_name: &str,
        entries: &[ConversationEntry],
    ) -> Result<(), AvixError> {
        let provider = match &self.local {
            Some(p) => p,
            None => return Ok(()),
        };
        let mut lines = String::new();
        for entry in entries {
            let line =
                serde_json::to_string(entry).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            lines.push_str(&line);
            lines.push('\n');
        }
        let rel = format!(
            "{}/agents/{}/invocations/{}/conversation.jsonl",
            username, agent_name, id
        );
        provider
            .write(&rel, lines.into_bytes())
            .await
            .map_err(|e| AvixError::Io(e.to_string()))
    }

    // ── Read operations ───────────────────────────────────────────────────────

    pub async fn get(&self, id: &str) -> Result<Option<InvocationRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table
            .get(id)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            Some(v) => {
                let record: InvocationRecord = serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    pub async fn list_for_user(&self, username: &str) -> Result<Vec<InvocationRecord>, AvixError> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(|r| r.username == username)
            .collect())
    }

    pub async fn list_for_agent(
        &self,
        username: &str,
        agent_name: &str,
    ) -> Result<Vec<InvocationRecord>, AvixError> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(|r| r.username == username && r.agent_name == agent_name)
            .collect())
    }

    /// Admin-only: returns all invocations across all users.
    pub async fn list_all(&self) -> Result<Vec<InvocationRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut records = Vec::new();
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let record: InvocationRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            records.push(record);
        }
        Ok(records)
    }

    // ── Disk artefact helpers ─────────────────────────────────────────────────

    async fn write_yaml_artefact(&self, record: &InvocationRecord) {
        let provider = match &self.local {
            Some(p) => p,
            None => return,
        };
        let yaml = match serde_yaml::to_string(record) {
            Ok(y) => y,
            Err(e) => {
                warn!(id = %record.id, "failed to serialize invocation YAML: {e}");
                return;
            }
        };
        let rel = format!(
            "{}/agents/{}/invocations/{}.yaml",
            record.username, record.agent_name, record.id
        );
        if let Err(e) = provider.write(&rel, yaml.into_bytes()).await {
            warn!(id = %record.id, "failed to write invocation YAML artefact: {e}");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_store() -> InvocationStore {
        let dir = tempdir().unwrap();
        InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
    }

    fn make_record(id: &str, username: &str, agent: &str) -> InvocationRecord {
        InvocationRecord::new(
            id.into(),
            agent.into(),
            username.into(),
            10,
            "do stuff".into(),
            "sess-1".into(),
        )
    }

    // T-INV-01
    #[tokio::test]
    async fn create_and_get_roundtrip() {
        let store = open_store().await;
        let rec = make_record("inv-001", "alice", "researcher");
        store.create(&rec).await.unwrap();
        let loaded = store.get("inv-001").await.unwrap().unwrap();
        assert_eq!(loaded.id, "inv-001");
        assert_eq!(loaded.agent_name, "researcher");
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.status, InvocationStatus::Running);
    }

    // T-INV-02
    #[tokio::test]
    async fn finalize_updates_status() {
        let store = open_store().await;
        let rec = make_record("inv-002", "alice", "coder");
        store.create(&rec).await.unwrap();
        store
            .finalize(
                "inv-002",
                InvocationStatus::Completed,
                Utc::now(),
                5000,
                12,
                None,
            )
            .await
            .unwrap();
        let loaded = store.get("inv-002").await.unwrap().unwrap();
        assert_eq!(loaded.status, InvocationStatus::Completed);
        assert!(loaded.ended_at.is_some());
        assert_eq!(loaded.tokens_consumed, 5000);
        assert_eq!(loaded.tool_calls_total, 12);
    }

    // T-INV-03
    #[tokio::test]
    async fn list_for_user_filters_by_username() {
        let store = open_store().await;
        store
            .create(&make_record("a", "alice", "bot"))
            .await
            .unwrap();
        store.create(&make_record("b", "bob", "bot")).await.unwrap();
        store
            .create(&make_record("c", "alice", "coder"))
            .await
            .unwrap();
        let alice = store.list_for_user("alice").await.unwrap();
        assert_eq!(alice.len(), 2);
        assert!(alice.iter().all(|r| r.username == "alice"));
    }

    // T-INV-04
    #[tokio::test]
    async fn list_for_agent_filters_by_agent() {
        let store = open_store().await;
        store
            .create(&make_record("a", "alice", "researcher"))
            .await
            .unwrap();
        store
            .create(&make_record("b", "alice", "coder"))
            .await
            .unwrap();
        let result = store.list_for_agent("alice", "researcher").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "a");
    }

    // T-INV-05
    #[tokio::test]
    async fn list_all_spans_users() {
        let store = open_store().await;
        store
            .create(&make_record("a", "alice", "bot"))
            .await
            .unwrap();
        store.create(&make_record("b", "bob", "bot")).await.unwrap();
        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    // T-INV-06
    #[tokio::test]
    async fn write_conversation_creates_jsonl() {
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-c", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let messages = vec![
            ("user".into(), "Hello agent".into()),
            ("assistant".into(), "Hello user".into()),
            ("user".into(), "Do something".into()),
        ];
        store
            .write_conversation("inv-c", "alice", "researcher", &messages)
            .await
            .unwrap();

        let path = dir
            .path()
            .join("alice/agents/researcher/invocations/inv-c/conversation.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["role"], "user");
        assert_eq!(parsed["content"], "Hello agent");
    }

    // T-INV-07
    #[tokio::test]
    async fn finalize_unknown_id_is_idempotent() {
        let store = open_store().await;
        let result = store
            .finalize(
                "does-not-exist",
                InvocationStatus::Completed,
                Utc::now(),
                0,
                0,
                None,
            )
            .await;
        assert!(result.is_ok());
    }

    // T-INV-08
    #[tokio::test]
    async fn two_invocations_for_same_agent_do_not_collide() {
        let store = open_store().await;
        store
            .create(&make_record("x1", "alice", "researcher"))
            .await
            .unwrap();
        store
            .create(&make_record("x2", "alice", "researcher"))
            .await
            .unwrap();
        let result = store.list_for_agent("alice", "researcher").await.unwrap();
        assert_eq!(result.len(), 2);
        let ids: Vec<&str> = result.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"x1"));
        assert!(ids.contains(&"x2"));
    }

    // T-INV-09
    #[tokio::test]
    async fn persist_interim_updates_tokens_and_tool_calls() {
        let store = open_store().await;
        let rec = make_record("inv-09", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let loaded = store.get("inv-09").await.unwrap().unwrap();
        assert_eq!(loaded.tokens_consumed, 0);
        assert_eq!(loaded.tool_calls_total, 0);

        store.persist_interim("inv-09", &[], 1500, 5).await.unwrap();

        let loaded = store.get("inv-09").await.unwrap().unwrap();
        assert_eq!(loaded.tokens_consumed, 1500);
        assert_eq!(loaded.tool_calls_total, 5);
        assert_eq!(loaded.status, InvocationStatus::Running);
        assert!(loaded.ended_at.is_none());
    }

    // T-INV-10
    #[tokio::test]
    async fn persist_interim_writes_conversation_partial() {
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-10", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let messages = vec![
            ("user".into(), "Hello agent".into()),
            ("assistant".into(), "Hello user".into()),
        ];
        store
            .persist_interim("inv-10", &messages, 100, 1)
            .await
            .unwrap();

        let path = dir
            .path()
            .join("alice/agents/researcher/invocations/inv-10/conversation.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    // T-INV-11
    #[tokio::test]
    async fn persist_interim_unknown_id_is_idempotent() {
        let store = open_store().await;
        let result = store.persist_interim("does-not-exist", &[], 0, 0).await;
        assert!(result.is_ok());
    }

    // T-INV-12
    #[tokio::test]
    async fn write_conversation_structured_creates_jsonl() {
        use super::super::conversation::{ConversationEntry, Role, ToolCallEntry};
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-12", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let entries = vec![
            ConversationEntry::from_role_content(Role::User, "Hello agent"),
            ConversationEntry {
                role: Role::Assistant,
                content: "Reading file".into(),
                tool_calls: vec![ToolCallEntry {
                    id: "call-1".into(),
                    name: "fs/read".into(),
                    args: serde_json::json!({"path": "/foo"}),
                    result: Some(serde_json::json!({"content": "bar"})),
                }],
                files_changed: vec![],
                thought: Some("checking file".into()),
            },
        ];
        store
            .write_conversation_structured("inv-12", "alice", "researcher", &entries)
            .await
            .unwrap();

        let path = dir
            .path()
            .join("alice/agents/researcher/invocations/inv-12/conversation.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(parsed["role"], "assistant");
        assert_eq!(parsed["toolCalls"].as_array().unwrap().len(), 1);
    }

    // T-INV-13
    #[tokio::test]
    async fn persist_interim_structured_writes_structured_jsonl() {
        use super::super::conversation::{ConversationEntry, FileDiffEntry, Role};
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-13", "alice", "coder");
        store.create(&rec).await.unwrap();

        let entries = vec![
            ConversationEntry::from_role_content(Role::User, "Write a function"),
            ConversationEntry {
                role: Role::Assistant,
                content: "Done".into(),
                tool_calls: vec![],
                files_changed: vec![FileDiffEntry {
                    path: "/foo.rs".into(),
                    diff: Some("+fn main() {}".into()),
                    content: None,
                }],
                thought: None,
            },
        ];
        store
            .persist_interim_structured("inv-13", &entries, 200, 3)
            .await
            .unwrap();

        let path = dir
            .path()
            .join("alice/agents/coder/invocations/inv-13/conversation.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let parsed: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(parsed["filesChanged"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["filesChanged"][0]["path"], "/foo.rs");

        let loaded = store.get("inv-13").await.unwrap().unwrap();
        assert_eq!(loaded.tokens_consumed, 200);
        assert_eq!(loaded.tool_calls_total, 3);
        assert_eq!(loaded.status, InvocationStatus::Running);
    }
}
