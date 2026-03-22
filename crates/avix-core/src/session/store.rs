use super::entry::SessionEntry;
use crate::error::AvixError;
use redb::{Database, TableDefinition};
use std::path::PathBuf;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("sessions");

pub struct SessionStore {
    db: Database,
}

impl SessionStore {
    pub async fn open(path: PathBuf) -> Result<Self, AvixError> {
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
        Ok(Self { db })
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
}
