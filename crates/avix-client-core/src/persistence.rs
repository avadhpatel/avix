use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

use crate::error::ClientError;
use crate::notification::Notification;

pub fn app_data_dir() -> PathBuf {
    let mut dir = std::env::var(\"HOME\").expect(\"$HOME not set\");
    dir.push_str(\"/.avix\");
    PathBuf::from(dir)
}

pub fn load_json<T>(path: &Path) -> Result<T, ClientError>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(Default::default());
    }
    let content = fs::read_to_string(path).map_err(|e| ClientError::Other(e.into()))?;
    serde_json::from_str(&content).map_err(|e| ClientError::Json(e))
}

pub fn save_json<T>(path: &Path, value: &T) -> Result<(), ClientError>
where
    T: Serialize,
{
    let tmp_path = path.with_extension(\"tmp\");
    let json = serde_json::to_string_pretty(value).map_err(ClientError::Json)?;
    fs::write(&tmp_path, json).map_err(|e| ClientError::Other(e.into()))?;
    fs::rename(&tmp_path, path).map_err(|e| ClientError::Other(e.into()))
}

pub fn notifications_path() -> PathBuf {
    app_data_dir().join(\"notifications.json\")
}

pub fn layout_path() -> PathBuf {
    app_data_dir().join(\"ui-layout.json\")
}

pub fn load_notifications() -> Result<Vec<Notification>, ClientError> {
    load_json(&notifications_path())
}

pub fn save_notifications(ns: &[Notification]) -> Result<(), ClientError> {
    save_json(&notifications_path(), ns)
}