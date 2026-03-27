use std::fs;
use std::path::{Path, PathBuf};
use tracing::debug;
use tracing::warn;

use serde::{de::DeserializeOwned, Serialize};
use serde_yaml;

use crate::error::ClientError;
use crate::notification::Notification;

pub fn app_data_dir() -> PathBuf {
    let dir = dirs::config_dir().map_or_else(
        || {
            warn!("Unable to determine config directory");
            panic!("Unable to determine config directory");
        },
        |p| p.join("avix"),
    );
    debug!("app_data_dir={:?}", dir);
    dir
}

pub fn load_json<T>(path: &Path) -> Result<T, ClientError>
where
    T: DeserializeOwned + Default,
{
    debug!("read {:?}", path);
    if !path.exists() {
        debug!("path doesn't exists");
        return Ok(Default::default());
    }
    let content = fs::read_to_string(path).map_err(|e| {
        warn!("Failed to read file {}: {}", path.display(), e);
        ClientError::Other(e.into())
    })?;
    serde_json::from_str(&content).map_err(|e| {
        warn!(
            "Failed to deserialize JSON from file {}: {}",
            path.display(),
            e
        );
        ClientError::Json(e)
    })
}

pub fn save_json<T>(path: &Path, value: &T) -> Result<(), ClientError>
where
    T: Serialize + ?Sized,
{
    let tmp_path = path.with_extension("tmp");
    let json = serde_json::to_string_pretty(value).map_err(|e| {
        warn!(
            "Failed to serialize value to JSON for {}: {}",
            path.display(),
            e
        );
        ClientError::Json(e)
    })?;
    debug!("write {:?} {}B", path, json.len());
    fs::write(&tmp_path, json).map_err(|e| {
        warn!("Failed to write to tmp file {}: {}", tmp_path.display(), e);
        ClientError::Other(e.into())
    })?;
    fs::rename(&tmp_path, path).map_err(|e| {
        warn!(
            "Failed to rename {} to {}: {}",
            tmp_path.display(),
            path.display(),
            e
        );
        ClientError::Other(e.into())
    })
}

pub fn load_yaml<T>(path: &Path) -> Result<T, ClientError>
where
    T: DeserializeOwned + Default,
{
    debug!("read {:?}", path);
    if !path.exists() {
        debug!("path doesn't exists");
        return Ok(Default::default());
    }
    let content = fs::read_to_string(path).map_err(|e| {
        warn!("Failed to read file {}: {}", path.display(), e);
        ClientError::Other(e.into())
    })?;
    serde_yaml::from_str(&content).map_err(|e| {
        warn!(
            "Failed to deserialize YAML from file {}: {}",
            path.display(),
            e
        );
        ClientError::Other(e.into())
    })
}

pub fn save_yaml<T>(path: &Path, value: &T) -> Result<(), ClientError>
where
    T: Serialize + ?Sized,
{
    let tmp_path = path.with_extension("tmp");
    let yaml = serde_yaml::to_string(value).map_err(|e| {
        warn!(
            "Failed to serialize value to YAML for {}: {}",
            path.display(),
            e
        );
        ClientError::Other(e.into())
    })?;
    debug!("write {:?} {}B", path, yaml.len());
    fs::write(&tmp_path, yaml).map_err(|e| {
        warn!("Failed to write to tmp file {}: {}", tmp_path.display(), e);
        ClientError::Other(e.into())
    })?;
    fs::rename(&tmp_path, path).map_err(|e| {
        warn!(
            "Failed to rename {} to {}: {}",
            tmp_path.display(),
            path.display(),
            e
        );
        ClientError::Other(e.into())
    })
}

pub fn notifications_path() -> PathBuf {
    app_data_dir().join("notifications.json")
}

pub fn layout_path() -> PathBuf {
    app_data_dir().join("ui-layout.json")
}

pub fn load_notifications() -> Result<Vec<Notification>, ClientError> {
    load_json(&notifications_path())
}

pub fn save_notifications(ns: &[Notification]) -> Result<(), ClientError> {
    save_json(&notifications_path(), ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::Notification;
    use tempfile::TempDir;

    #[test]
    fn save_and_load_notifications_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("notifications.json");
        let ns = vec![
            Notification::from_sys_alert("info", "test1"),
            Notification::from_sys_alert("warn", "test2"),
        ];
        save_json(&path, &ns).unwrap();
        let loaded: Vec<Notification> = load_json(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].message, "test1");
        assert_eq!(loaded[1].message, "test2");
    }

    #[test]
    fn load_json_returns_default_if_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.json");
        let loaded: Vec<Notification> = load_json(&path).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn atomic_write_does_not_leave_tmp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.json");
        let data = vec!["hello"];
        save_json(&path, &data).unwrap();
        assert!(!dir.path().join("data.json.tmp").exists());
        assert!(path.exists());
    }
}
