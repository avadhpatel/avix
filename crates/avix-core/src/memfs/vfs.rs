use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::path::VfsPath;
use crate::error::AvixError;

#[derive(Debug, Default)]
pub struct MemFs {
    files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn write(&self, path: &VfsPath, content: Vec<u8>) -> Result<(), AvixError> {
        self.files
            .write()
            .await
            .insert(path.as_str().to_string(), content);
        Ok(())
    }

    pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError> {
        self.files
            .read()
            .await
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| AvixError::ConfigParse(format!("ENOENT: {}", path.as_str())))
    }

    pub async fn delete(&self, path: &VfsPath) -> Result<(), AvixError> {
        self.files
            .write()
            .await
            .remove(path.as_str())
            .ok_or_else(|| AvixError::ConfigParse(format!("ENOENT: {}", path.as_str())))?;
        Ok(())
    }

    pub async fn exists(&self, path: &VfsPath) -> bool {
        self.files.read().await.contains_key(path.as_str())
    }

    pub async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError> {
        let prefix = format!("{}/", dir.as_str().trim_end_matches('/'));
        let guard = self.files.read().await;
        let entries: Vec<String> = guard
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .filter_map(|k| {
                let rest = &k[prefix.len()..];
                // Only immediate children (no '/' in the remainder)
                if rest.contains('/') {
                    None
                } else {
                    Some(rest.to_string())
                }
            })
            .collect();
        if entries.is_empty() {
            // Check if any file exists under this dir at all
            let any = guard.keys().any(|k| k.starts_with(&prefix));
            if !any {
                return Err(AvixError::ConfigParse(format!("ENOENT: {}", dir.as_str())));
            }
        }
        Ok(entries)
    }
}
