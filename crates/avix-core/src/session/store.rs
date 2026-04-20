use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, TableDefinition};
use serde::Serialize;

use super::entry::{AgentRef, QuotaSnapshot, SessionEntry, SessionState};
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
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
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
        let view = SessionManifestView::from(entry);
        let yaml = match serde_yaml::to_string(&view) {
            Ok(y) => y,
            Err(_) => return,
        };
        let path_str = format!(
            "/proc/users/{}/sessions/{}.yaml",
            entry.username, entry.session_id
        );
        if let Ok(path) = VfsPath::parse(&path_str) {
            let _ = vfs.write(&path, yaml.into_bytes()).await;
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

// ── VFS manifest view (spec-compliant schema, excludes redb-internal fields) ─

#[derive(Serialize)]
struct SessionManifestView<'a> {
    #[serde(rename = "apiVersion")]
    api_version: &'static str,
    kind: &'static str,
    metadata: ManifestMeta<'a>,
    spec: ManifestSpec<'a>,
    status: ManifestStatus<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestMeta<'a> {
    session_id: &'a str,
    created_at: &'a chrono::DateTime<chrono::Utc>,
    user: &'a str,
    uid: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestSpec<'a> {
    shell: &'a str,
    tty: bool,
    working_directory: &'a str,
    agents: &'a [AgentRef],
    quota_snapshot: &'a QuotaSnapshot,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestStatus<'a> {
    state: &'a SessionState,
    last_activity_at: &'a chrono::DateTime<chrono::Utc>,
    closed_at: Option<&'a chrono::DateTime<chrono::Utc>>,
    closed_reason: Option<&'a str>,
}

impl<'a> From<&'a SessionEntry> for SessionManifestView<'a> {
    fn from(e: &'a SessionEntry) -> Self {
        Self {
            api_version: "avix/v1",
            kind: "SessionManifest",
            metadata: ManifestMeta {
                session_id: &e.session_id,
                created_at: &e.created_at,
                user: &e.username,
                uid: e.uid,
            },
            spec: ManifestSpec {
                shell: &e.shell,
                tty: e.tty,
                working_directory: &e.working_directory,
                agents: &e.agents,
                quota_snapshot: &e.quota_snapshot,
            },
            status: ManifestStatus {
                state: &e.state,
                last_activity_at: &e.last_activity_at,
                closed_at: e.closed_at.as_ref(),
                closed_reason: e.closed_reason.as_deref(),
            },
        }
    }
}
