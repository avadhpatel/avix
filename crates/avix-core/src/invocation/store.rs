use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::{debug, instrument, warn};

use super::conversation::ConversationEntry;
use super::record::{InvocationRecord, InvocationStatus};
use crate::error::AvixError;
use crate::memfs::local_provider::LocalProvider;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("invocations");

/// Persistent store for agent invocation records.
///
/// Primary store: `redb` — fast keyed lookups for list/get operations.
/// JSONL conversation: `LocalProvider` — written to
/// `<username>/.sessions/<session_id>/<pid>.jsonl` (keyed by PID, reused across turns).
///
/// The `local` provider is optional. When absent, only redb is used (useful in tests).
#[derive(Debug)]
pub struct InvocationStore {
    db: Arc<Database>,
    local: Option<Arc<LocalProvider>>,
}

impl InvocationStore {
    #[instrument]
    pub async fn open(path: impl Into<PathBuf> + std::fmt::Debug) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
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
            db: Arc::new(db),
            local: None,
        })
    }

    /// Attach a `LocalProvider` rooted at `<avix_root>/data/users/` for JSONL artefacts.
    #[instrument]
    pub fn with_local(mut self, provider: LocalProvider) -> Self {
        self.local = Some(Arc::new(provider));
        self
    }

    // ── Write operations ──────────────────────────────────────────────────────

    /// Persist a new `InvocationRecord` (status: Running).
    #[instrument]
    pub async fn create(&self, record: &InvocationRecord) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = &self.db;
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
        debug!(id = %record.id, pid = record.pid, "invocation record created");
        Ok(())
    }

    /// Update a record's terminal fields after the agent exits.
    ///
    /// Idempotent: silently succeeds if `id` is not found.
    #[instrument]
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
        let db = &self.db;
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
        debug!(id, status = ?record.status, "invocation finalized");
        Ok(())
    }

    /// Update only the status of a record (e.g., transition to Idle).
    #[instrument]
    pub async fn update_status(&self, id: &str, status: InvocationStatus) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        record.status = status;

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = &self.db;
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
        Ok(())
    }

    /// Update the goal (command) of an existing record — called when SIGSTART
    /// delivers a new command to an idle executor before it starts the next turn.
    #[instrument]
    pub async fn update_goal(&self, id: &str, goal: &str) -> Result<(), AvixError> {
        let mut record = match self.get(id).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        record.goal = goal.to_string();

        let json =
            serde_json::to_string(&record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = &self.db;
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
        Ok(())
    }

    /// Write interim snapshot of a running invocation (redb + JSONL).
    ///
    /// Does NOT set ended_at or change status. Updates tokens/tool_calls.
    /// Idempotent: silently succeeds if `id` is not found.
    #[instrument]
    pub async fn persist_interim(
        &self,
        id: &str,
        conversation: &[ConversationEntry],
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
        let db = &self.db;
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

        if !conversation.is_empty() {
            self.write_conversation_structured(
                record.pid,
                &record.session_id,
                &record.username,
                conversation,
            )
            .await?;
        }

        Ok(())
    }

    /// Write interim snapshot with structured conversation entries.
    ///
    /// Does NOT set ended_at or change status. Updates tokens/tool_calls.
    /// Idempotent: silently succeeds if `id` is not found.
    #[instrument]
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
        let db = &self.db;
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

        if !entries.is_empty() {
            self.write_conversation_structured(
                record.pid,
                &record.session_id,
                &record.username,
                entries,
            )
            .await?;
        }

        Ok(())
    }

    /// Write structured conversation entries as a JSONL file.
    ///
    /// Written to `<username>/.sessions/<session_id>/<pid>.jsonl`.
    /// Overwrites the full file — call with the complete history each time.
    #[instrument]
    pub async fn write_conversation_structured(
        &self,
        pid: u64,
        session_id: &str,
        username: &str,
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
        let rel = format!("{}/.sessions/{}/{}.jsonl", username, session_id, pid);
        debug!(pid, session_id, username, path = %rel, "writing conversation JSONL");
        provider
            .write(&rel, lines.into_bytes())
            .await
            .map_err(|e| AvixError::Io(e.to_string()))
    }

    // ── Read operations ───────────────────────────────────────────────────────

    #[instrument]
    pub async fn get(&self, id: &str) -> Result<Option<InvocationRecord>, AvixError> {
        let db = &self.db;
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

    #[instrument]
    pub async fn list_for_user(&self, username: &str) -> Result<Vec<InvocationRecord>, AvixError> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(|r| r.username == username)
            .collect())
    }

    #[instrument]
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

    #[instrument]
    pub async fn list_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<InvocationRecord>, AvixError> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(|r| r.session_id == session_id)
            .collect())
    }

    /// Read the JSONL conversation for an invocation.
    ///
    /// File lives at `<username>/.sessions/<session_id>/<pid>.jsonl`.
    /// Returns an empty vec if the file does not exist (pre-first-turn invocations).
    #[instrument]
    pub async fn read_conversation(
        &self,
        session_id: &str,
        pid: u64,
        username: &str,
    ) -> Result<Vec<ConversationEntry>, AvixError> {
        let provider = match &self.local {
            Some(p) => p,
            None => return Ok(vec![]),
        };
        let rel = format!("{}/.sessions/{}/{}.jsonl", username, session_id, pid);
        debug!(session_id, pid, username, path = %rel, "reading conversation JSONL");
        let bytes = match provider.read(&rel).await {
            Ok(b) => b,
            Err(_) => return Ok(vec![]),
        };
        let text = String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ConversationEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!(error = %e, session_id, pid, "skipping malformed conversation line");
                }
            }
        }
        Ok(entries)
    }

    /// Admin-only: returns all invocations across all users.
    #[instrument]
    pub async fn list_all(&self) -> Result<Vec<InvocationRecord>, AvixError> {
        let db = &self.db;
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

    // T-INV-06: write_conversation_structured creates JSONL at new path
    #[tokio::test]
    async fn write_conversation_structured_creates_jsonl_at_session_path() {
        use super::super::conversation::{ConversationEntry, Role};
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-c", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let entries = vec![
            ConversationEntry::from_role_content(Role::User, "Hello agent"),
            ConversationEntry::from_role_content(Role::Assistant, "Hello user"),
        ];
        // pid=10, session_id="sess-1" from make_record
        store
            .write_conversation_structured(10, "sess-1", "alice", &entries)
            .await
            .unwrap();

        let path = dir.path().join("alice/.sessions/sess-1/10.jsonl");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
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
    async fn persist_interim_writes_conversation_at_session_path() {
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-10", "alice", "researcher");
        store.create(&rec).await.unwrap();

        use super::super::conversation::Role;
        let messages = vec![
            ConversationEntry::from_role_content(Role::User, "Hello agent"),
            ConversationEntry::from_role_content(Role::Assistant, "Hello user"),
        ];
        store
            .persist_interim("inv-10", &messages, 100, 1)
            .await
            .unwrap();

        // pid=10, session_id="sess-1" from make_record
        let path = dir.path().join("alice/.sessions/sess-1/10.jsonl");
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
    async fn write_conversation_structured_with_tool_calls() {
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
            .write_conversation_structured(10, "sess-1", "alice", &entries)
            .await
            .unwrap();

        let path = dir.path().join("alice/.sessions/sess-1/10.jsonl");
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

        let path = dir.path().join("alice/.sessions/sess-1/10.jsonl");
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

    // T-INV-14
    #[tokio::test]
    async fn list_for_session_filters_by_session_id() {
        let store = open_store().await;
        store
            .create(&make_record("a", "alice", "bot"))
            .await
            .unwrap();
        let mut rec2 = make_record("b", "alice", "bot");
        rec2.session_id = "sess-2".into();
        store.create(&rec2).await.unwrap();
        store
            .create(&make_record("c", "alice", "coder"))
            .await
            .unwrap();

        let results = store.list_for_session("sess-1").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.session_id == "sess-1"));

        let results2 = store.list_for_session("sess-2").await.unwrap();
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].id, "b");

        let empty = store.list_for_session("no-such-session").await.unwrap();
        assert!(empty.is_empty());
    }

    // T-INV-15
    #[tokio::test]
    async fn read_conversation_roundtrip() {
        use super::super::conversation::{ConversationEntry, Role};
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let rec = make_record("inv-15", "alice", "researcher");
        store.create(&rec).await.unwrap();

        let entries = vec![
            ConversationEntry::from_role_content(Role::User, "hello"),
            ConversationEntry::from_role_content(Role::Assistant, "world"),
        ];
        // pid=10, session_id="sess-1"
        store
            .write_conversation_structured(10, "sess-1", "alice", &entries)
            .await
            .unwrap();

        let loaded = store
            .read_conversation("sess-1", 10, "alice")
            .await
            .unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "hello");
        assert_eq!(loaded[1].content, "world");
    }

    // T-INV-16
    #[tokio::test]
    async fn read_conversation_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let provider = LocalProvider::new(dir.path()).unwrap();
        let store = InvocationStore::open(dir.path().join("inv.redb"))
            .await
            .unwrap()
            .with_local(provider);

        let result = store
            .read_conversation("no-such-session", 99, "alice")
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    // T-INV-17: agent_version roundtrips through redb
    #[tokio::test]
    async fn agent_version_roundtrips() {
        let store = open_store().await;
        let mut rec = make_record("inv-17", "alice", "researcher");
        rec.agent_version = "2.1.0".into();
        store.create(&rec).await.unwrap();
        let loaded = store.get("inv-17").await.unwrap().unwrap();
        assert_eq!(loaded.agent_version, "2.1.0");
    }
}
