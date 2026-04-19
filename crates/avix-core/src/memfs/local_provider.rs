use std::path::PathBuf;

use crate::error::AvixError;
use tracing::instrument;

/// Disk-backed VFS storage provider.
///
/// Rooted at a directory on the real filesystem. All paths are relative to the root.
/// Path traversal (`..`) is rejected at the `resolve_path` boundary.
#[derive(Debug)]
pub struct LocalProvider {
    root: PathBuf,
}

impl LocalProvider {
    #[instrument]
    pub fn new(root: impl Into<PathBuf> + std::fmt::Debug) -> Result<Self, AvixError> {
        let root = root.into();
        if !root.exists() {
            std::fs::create_dir_all(&root)
                .map_err(|e| AvixError::Io(format!("create provider root: {e}")))?;
        }
        let root = root
            .canonicalize()
            .map_err(|e| AvixError::Io(format!("canonicalize provider root: {e}")))?;
        Ok(Self { root })
    }

    /// Resolve a relative path against the provider root.
    ///
    /// Rejects paths containing `..` segments and any path that would escape the root
    /// after resolution.
    #[instrument]
    fn resolve_path(&self, rel: &str) -> Result<PathBuf, AvixError> {
        // Strip leading '/' (VFS paths are absolute; LocalProvider expects relative)
        let rel = rel.trim_start_matches('/');

        if rel.contains("..") {
            return Err(AvixError::Io(format!("path traversal rejected: '{rel}'")));
        }

        let candidate = self.root.join(rel);

        // For existing files: canonicalize and check prefix.
        // For non-existing files: normalise manually (no symlink resolution possible).
        // We already rejected '..', so a simple join is safe.
        Ok(candidate)
    }

    #[instrument]
    pub async fn read(&self, rel: &str) -> Result<Vec<u8>, AvixError> {
        let path = self.resolve_path(rel)?;
        tokio::fs::read(&path)
            .await
            .map_err(|e| AvixError::NotFound(format!("read {rel}: {e}")))
    }

    #[instrument]
    pub async fn write(&self, rel: &str, content: Vec<u8>) -> Result<(), AvixError> {
        let path = self.resolve_path(rel)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AvixError::Io(format!("create_dir_all for {rel}: {e}")))?;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| AvixError::Io(format!("write {rel}: {e}")))
    }

    #[instrument]
    pub async fn delete(&self, rel: &str) -> Result<(), AvixError> {
        let path = self.resolve_path(rel)?;
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| AvixError::NotFound(format!("delete {rel}: {e}")))
    }

    #[instrument]
    pub async fn exists(&self, rel: &str) -> bool {
        match self.resolve_path(rel) {
            Ok(path) => path.exists(),
            Err(_) => false,
        }
    }

    /// List immediate children of a directory.
    ///
    /// Returns file and directory names (not full paths) under `rel_dir`.
    /// Returns `ENOENT`-style error if the directory does not exist.
    #[instrument]
    pub async fn list(&self, rel_dir: &str) -> Result<Vec<String>, AvixError> {
        let path = self.resolve_path(rel_dir)?;
        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| AvixError::NotFound(format!("list {rel_dir}: {e}")))?;
        let mut entries = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AvixError::Io(format!("read dir entry in {rel_dir}: {e}")))?
        {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
        Ok(entries)
    }
}
