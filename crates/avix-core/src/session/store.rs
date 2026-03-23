use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, TableDefinition};

use super::entry::{SessionEntry, SessionStatus};
use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("sessions");

pub struct SessionStore {
    db: Database,
    vfs: Option<Arc<VfsRouter>>,
}

impl SessionStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        // Ensure table exists
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
        Ok(Self { db, vfs: None })
    }

    pub fn with_vfs(mut self, vfs: Arc<VfsRouter>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    pub async fn save(&self, entry: &SessionEntry) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(entry).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(entry.session_id.as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_vfs_manifest(entry).await;
        Ok(())
    }

    pub async fn load(&self, session_id: &str) -> Result<Option<SessionEntry>, AvixError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table
            .get(session_id)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            Some(v) => {
                let entry = serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    pub async fn delete(&self, session_id: &str) -> Result<(), AvixError> {
        // Load username before deleting so we can clean up the VFS entry
        let username = self
            .load(session_id)
            .await?
            .map(|e| e.username)
            .unwrap_or_default();

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .remove(session_id)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.remove_vfs_manifest(session_id, &username).await;
        Ok(())
    }

    pub async fn list_all(&self) -> Result<Vec<SessionEntry>, AvixError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        use redb::ReadableTable;
        let mut entries = Vec::new();
        let iter = table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        for item in iter {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let entry: SessionEntry = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            entries.push(entry);
        }
        Ok(entries)
    }

    // ── VFS manifest helpers ──────────────────────────────────────────────────

    async fn write_vfs_manifest(&self, entry: &SessionEntry) {
        let vfs = match &self.vfs {
            Some(v) => v,
            None => return,
        };
        if entry.username.is_empty() {
            return;
        }
        let message_count = entry.messages.len();
        let status_str = match entry.status {
            SessionStatus::Active => "active",
            SessionStatus::Completed => "completed",
            SessionStatus::Error => "error",
        };
        let manifest = format!(
            "apiVersion: avix/v1\nkind: SessionManifest\nmetadata:\n  sessionId: {id}\n  username: {username}\n  createdAt: {created}\n  updatedAt: {updated}\nspec:\n  agentName: {agent}\n  goal: {goal:?}\n  status: {status}\n  messageCount: {message_count}\n",
            id = entry.session_id,
            username = entry.username,
            created = entry.created_at.to_rfc3339(),
            updated = entry.updated_at.to_rfc3339(),
            agent = entry.agent_name,
            goal = entry.goal,
            status = status_str,
        );
        let path_str = format!(
            "/proc/users/{}/sessions/{}.yaml",
            entry.username, entry.session_id
        );
        if let Ok(path) = VfsPath::parse(&path_str) {
            let _ = vfs.write(&path, manifest.into_bytes()).await;
        }
    }

    async fn remove_vfs_manifest(&self, session_id: &str, username: &str) {
        let vfs = match &self.vfs {
            Some(v) => v,
            None => return,
        };
        if username.is_empty() {
            return;
        }
        let path_str = format!("/proc/users/{username}/sessions/{session_id}.yaml");
        if let Ok(path) = VfsPath::parse(&path_str) {
            let _ = vfs.delete(&path).await;
        }
    }
}
