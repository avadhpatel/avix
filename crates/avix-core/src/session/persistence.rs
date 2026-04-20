use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::{debug, instrument};

use super::record::SessionRecord;
#[cfg(test)]
use super::record::SessionStatus;
use crate::error::AvixError;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("sessions");

#[derive(Debug)]
pub struct SessionStore {
    db: Arc<Database>,
}

impl SessionStore {
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
        })
    }

    #[instrument]
    pub async fn create(&self, record: &SessionRecord) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let write_txn = self.db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        debug!(session_id = %record.id, username = %record.username, "session created");
        Ok(())
    }

    #[instrument]
    pub async fn get(&self, id: &uuid::Uuid) -> Result<Option<SessionRecord>, AvixError> {
        let read_txn = self.db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table
            .get(id.to_string().as_str())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            Some(v) => {
                let record: SessionRecord = serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    #[instrument]
    pub async fn update(&self, record: &SessionRecord) -> Result<(), AvixError> {
        let json =
            serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let write_txn = self.db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        debug!(session_id = %record.id, status = ?record.status, "session updated");
        Ok(())
    }

    #[instrument]
    pub async fn delete(&self, id: &uuid::Uuid) -> Result<(), AvixError> {
        let write_txn = self.db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn
                .open_table(TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .remove(id.to_string().as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        debug!(session_id = %id, "session deleted");
        Ok(())
    }

    #[instrument]
    pub async fn list_for_user(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        let read_txn = self.db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut entries = Vec::new();
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let record: SessionRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            if record.username == username {
                entries.push(record);
            }
        }
        Ok(entries)
    }

    #[instrument]
    pub async fn list_all(&self) -> Result<Vec<SessionRecord>, AvixError> {
        let read_txn = self.db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let mut entries = Vec::new();
        for item in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let record: SessionRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            entries.push(record);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use uuid::Uuid;

    async fn open_store() -> SessionStore {
        let dir = tempdir().unwrap();
        SessionStore::open(dir.path().join("sess.redb"))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_saves_record_to_redb() {
        let store = open_store().await;
        let record = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "researcher".to_string(),
            "Test Session".to_string(),
            "Analyze data".to_string(),
            1,
        );
        store.create(&record).await.unwrap();
        let loaded = store.get(&record.id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().id, record.id);
    }

    #[tokio::test]
    async fn get_returns_none_for_missing() {
        let store = open_store().await;
        let loaded = store.get(&Uuid::new_v4()).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn update_modifies_existing_record() {
        let store = open_store().await;
        let mut record = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "researcher".to_string(),
            "Test Session".to_string(),
            "Analyze data".to_string(),
            1,
        );
        store.create(&record).await.unwrap();

        record.mark_idle();
        record.summary = Some("Done with analysis".to_string());
        store.update(&record).await.unwrap();

        let loaded = store.get(&record.id).await.unwrap().unwrap();
        assert_eq!(loaded.status, SessionStatus::Idle);
        assert_eq!(loaded.summary, Some("Done with analysis".to_string()));
    }

    #[tokio::test]
    async fn list_for_user_filters_by_username() {
        let store = open_store().await;

        let r1 = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "a1".to_string(),
            "s1".to_string(),
            "g1".to_string(),
            2,
        );
        let r2 = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "a2".to_string(),
            "s2".to_string(),
            "g2".to_string(),
            3,
        );
        let r3 = SessionRecord::new(
            Uuid::new_v4(),
            "bob".to_string(),
            "b1".to_string(),
            "s3".to_string(),
            "g3".to_string(),
            4,
        );

        store.create(&r1).await.unwrap();
        store.create(&r2).await.unwrap();
        store.create(&r3).await.unwrap();

        let alice_sessions = store.list_for_user("alice").await.unwrap();
        assert_eq!(alice_sessions.len(), 2);

        let bob_sessions = store.list_for_user("bob").await.unwrap();
        assert_eq!(bob_sessions.len(), 1);
    }

    #[tokio::test]
    async fn invocation_pids_roundtrip() {
        use crate::session::record::PidInvocationMeta;
        let store = open_store().await;
        let mut record = SessionRecord::new(
            Uuid::new_v4(),
            "alice".to_string(),
            "researcher".to_string(),
            "Test".to_string(),
            "goal".to_string(),
            42,
        );
        record.add_invocation_pid(PidInvocationMeta {
            pid: 42,
            invocation_id: "inv-1".to_string(),
            agent_name: "researcher".to_string(),
            agent_version: "1.0.0".to_string(),
            spawned_at: chrono::Utc::now(),
        });
        store.create(&record).await.unwrap();
        let loaded = store.get(&record.id).await.unwrap().unwrap();
        assert_eq!(loaded.invocation_pids.len(), 1);
        assert_eq!(loaded.invocation_pids[0].agent_version, "1.0.0");
    }
}
