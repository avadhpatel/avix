use std::path::PathBuf;
use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;

use super::record::SessionRecord;
#[cfg(test)]
use super::record::SessionStatus;
use crate::error::AvixError;
use crate::memfs::local_provider::LocalProvider;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("sessions");

pub struct SessionStore {
    db: Arc<Mutex<Database>>,
    local: Option<Arc<LocalProvider>>,
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
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            local: None,
        })
    }

    pub fn with_local(mut self, provider: LocalProvider) -> Self {
        self.local = Some(Arc::new(provider));
        self
    }

    pub async fn create(&self, record: &SessionRecord) -> Result<(), AvixError> {
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
                .insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(record).await;
        Ok(())
    }

    pub async fn get(&self, id: &uuid::Uuid) -> Result<Option<SessionRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
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

    pub async fn update(&self, record: &SessionRecord) -> Result<(), AvixError> {
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
                .insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(record).await;
        Ok(())
    }

    pub async fn delete(&self, id: &uuid::Uuid) -> Result<(), AvixError> {
        let username = self.get(id).await?.map(|e| e.username).unwrap_or_default();
        let db = self.db.lock().await;
        let write_txn = db
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
        self.remove_yaml_artefact(id, &username).await;
        Ok(())
    }

    pub async fn list_for_user(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
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

    pub async fn list_all(&self) -> Result<Vec<SessionRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
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

    async fn write_yaml_artefact(&self, record: &SessionRecord) {
        let provider = match &self.local {
            Some(p) => p,
            None => return,
        };
        if record.username.is_empty() {
            return;
        }
        let yaml = match serde_yaml::to_string(record) {
            Ok(y) => y,
            Err(_) => return,
        };
        let rel = format!("{}/sessions/{}/session.yaml", record.username, record.id);
        let _ = provider.write(&rel, yaml.into_bytes()).await;
    }

    async fn remove_yaml_artefact(&self, id: &uuid::Uuid, username: &str) {
        let provider = match &self.local {
            Some(p) => p,
            None => return,
        };
        if username.is_empty() {
            return;
        }
        let rel = format!("{}/sessions/{}/session.yaml", username, id);
        let _ = provider.delete(&rel).await;
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
    async fn delete_removes_record() {
        let store = open_store().await;
        let id = Uuid::new_v4();
        let record = SessionRecord::new(
            id,
            "alice".to_string(),
            "a1".to_string(),
            "s1".to_string(),
            "g1".to_string(),
            5,
        );
        store.create(&record).await.unwrap();

        store.delete(&id).await.unwrap();

        let loaded = store.get(&id).await.unwrap();
        assert!(loaded.is_none());
    }
}
