use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::path::VfsPath;
use crate::error::AvixError;

#[derive(Debug, Default)]
pub struct MemFs {
    pub files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
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

        // Collect the first path component for every key under `prefix`.
        // This returns both direct file children and subdirectory names, which
        // is the standard `ls`-equivalent behaviour.
        let mut seen = std::collections::HashSet::new();
        for k in guard.keys().filter(|k| k.starts_with(&prefix)) {
            let rest = &k[prefix.len()..];
            let first = rest.split('/').next().unwrap_or(rest);
            if !first.is_empty() {
                seen.insert(first.to_string());
            }
        }

        if seen.is_empty() {
            return Err(AvixError::ConfigParse(format!("ENOENT: {}", dir.as_str())));
        }
        Ok(seen.into_iter().collect())
    }
}
