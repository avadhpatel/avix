use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

use super::schema::{MemoryRecord, UserPreferenceModel};

use tracing::instrument;

#[instrument]
pub async fn write_record(
    vfs: &VfsRouter,
    path: &str,
    record: &MemoryRecord,
) -> Result<(), AvixError> {
    let yaml = record.to_yaml()?;
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    vfs.write(&vfs_path, yaml.into_bytes()).await
}

#[instrument]
pub async fn read_record(vfs: &VfsRouter, path: &str) -> Result<MemoryRecord, AvixError> {
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let bytes = vfs
        .read(&vfs_path)
        .await
        .map_err(|_| AvixError::NotFound(format!("memory record not found: {path}")))?;
    let yaml = String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    MemoryRecord::from_yaml(&yaml)
}

#[instrument]
pub async fn delete_record(vfs: &VfsRouter, path: &str) -> Result<(), AvixError> {
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    vfs.delete(&vfs_path).await
}

/// List and parse all `.yaml` records (non-.keep) in a VFS directory.
#[instrument]
pub async fn list_records(vfs: &VfsRouter, dir: &str) -> Result<Vec<MemoryRecord>, AvixError> {
    let vfs_path = VfsPath::parse(dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let entries = vfs.list(&vfs_path).await.unwrap_or_default();
    let mut records = Vec::new();
    for entry in entries {
        if entry.ends_with(".yaml") && entry != ".keep" {
            let full_path = format!("{}/{}", dir.trim_end_matches('/'), entry);
            if let Ok(record) = read_record(vfs, &full_path).await {
                records.push(record);
            }
        }
    }
    Ok(records)
}

// ── Preference model helpers ───────────────────────────────────────────────────

#[instrument]
pub async fn read_preference_model(
    vfs: &VfsRouter,
    path: &str,
) -> Result<UserPreferenceModel, AvixError> {
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let bytes = vfs
        .read(&vfs_path)
        .await
        .map_err(|_| AvixError::NotFound(format!("preference model not found: {path}")))?;
    let yaml = String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    UserPreferenceModel::from_yaml(&yaml)
}

#[instrument]
pub async fn write_preference_model(
    vfs: &VfsRouter,
    path: &str,
    model: &UserPreferenceModel,
) -> Result<(), AvixError> {
    let yaml = model.to_yaml()?;
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    vfs.write(&vfs_path, yaml.into_bytes()).await
}
