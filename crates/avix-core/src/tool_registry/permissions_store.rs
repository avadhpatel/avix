use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::{Database, ReadableTable, TableDefinition};
use tokio::sync::Mutex;
use tracing::{info, instrument};

use crate::error::AvixError;

use super::permissions::ToolPermissions;

const TOOL_PERMS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("tool_permissions");

#[derive(Debug)]
pub struct ToolPermissionsStore {
    db: Arc<Mutex<Database>>,
    path: PathBuf,
}

impl ToolPermissionsStore {
    #[instrument]
    pub async fn open(root: &Path) -> Result<Self, AvixError> {
        let db_path = root.join("kernel/permissions.db");
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let db = Database::create(&db_path)
            .map_err(|e| AvixError::ConfigParse(format!("failed to open permissions.db: {}", e)))?;

        {
            let write_txn = db
                .begin_write()
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            write_txn
                .open_table(TOOL_PERMS_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            write_txn
                .commit()
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }

        info!(path = %db_path.display(), "tool permissions store opened");

        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            path: db_path,
        })
    }

    #[instrument]
    pub async fn get(&self, tool_name: &str) -> Result<Option<ToolPermissions>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TOOL_PERMS_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        if let Some(value) = table
            .get(tool_name)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let perms: ToolPermissions = serde_yaml::from_str(value.value()).map_err(|e| {
                AvixError::ConfigParse(format!("failed to parse permissions: {}", e))
            })?;
            Ok(Some(perms))
        } else {
            Ok(None)
        }
    }

    #[instrument]
    pub async fn set(&self, tool_name: &str, perms: &ToolPermissions) -> Result<(), AvixError> {
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let yaml = serde_yaml::to_string(perms).map_err(|e| {
            AvixError::ConfigParse(format!("failed to serialize permissions: {}", e))
        })?;

        {
            let mut table = write_txn
                .open_table(TOOL_PERMS_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .insert(tool_name, yaml.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }

        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(())
    }

    #[instrument]
    pub async fn list_all(&self) -> Result<Vec<(String, ToolPermissions)>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db
            .begin_read()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn
            .open_table(TOOL_PERMS_TABLE)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let mut results = Vec::new();
        for entry in table
            .iter()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let (key, value) = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let perms: ToolPermissions = serde_yaml::from_str(value.value())
                .map_err(|e| AvixError::ConfigParse(format!("failed to parse: {}", e)))?;
            results.push((key.value().to_string(), perms));
        }
        Ok(results)
    }

    #[instrument]
    pub async fn delete(&self, tool_name: &str) -> Result<(), AvixError> {
        let db = self.db.lock().await;
        let write_txn = db
            .begin_write()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        {
            let mut table = write_txn
                .open_table(TOOL_PERMS_TABLE)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table
                .remove(tool_name)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }

        write_txn
            .commit()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(())
    }

    #[instrument]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
