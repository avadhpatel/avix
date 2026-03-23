use tokio::sync::RwLock;

use super::local_provider::LocalProvider;
use super::path::VfsPath;
use super::vfs::MemFs;
use crate::error::AvixError;

/// VFS router that dispatches calls to a disk-backed `LocalProvider` for mounted
/// paths and falls back to an in-memory `MemFs` for everything else.
///
/// Mounts are matched by longest-prefix: `/users/alice` beats `/users` for the
/// path `/users/alice/defaults.yaml`.
///
/// Public API is intentionally identical to `MemFs` so call-sites can substitute
/// `Arc<VfsRouter>` for `Arc<MemFs>` mechanically.
pub struct VfsRouter {
    /// Sorted descending by prefix length so the longest match is found first.
    mounts: RwLock<Vec<(String, LocalProvider)>>,
    default: MemFs,
}

impl std::fmt::Debug for VfsRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsRouter").finish()
    }
}

impl Default for VfsRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            mounts: RwLock::new(Vec::new()),
            default: MemFs::new(),
        }
    }

    /// Add a `LocalProvider` that handles all VFS paths starting with `prefix`.
    ///
    /// The prefix must start with `/` and must not end with `/`.
    /// Adding a mount with a prefix that already exists replaces it.
    pub async fn mount(&self, prefix: String, provider: LocalProvider) {
        let prefix = prefix.trim_end_matches('/').to_string();
        let mut mounts = self.mounts.write().await;
        // Remove any existing entry with the same prefix
        mounts.retain(|(p, _)| p != &prefix);
        mounts.push((prefix, provider));
        // Keep sorted descending by prefix length (longest match first)
        mounts.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()));
    }

    // ── Routing helpers ───────────────────────────────────────────────────────

    /// Find the LocalProvider whose prefix is the longest match for `path`.
    /// Returns `(provider_ref, relative_path_within_provider)` or `None`.
    async fn route<'a>(
        &'a self,
        path: &str,
        mounts: &'a tokio::sync::RwLockReadGuard<'a, Vec<(String, LocalProvider)>>,
    ) -> Option<(&'a LocalProvider, String)> {
        for (prefix, provider) in mounts.iter() {
            if path == prefix.as_str() || path.starts_with(&format!("{prefix}/")) {
                let rel = path[prefix.len()..].trim_start_matches('/').to_string();
                return Some((provider, rel));
            }
        }
        None
    }

    // ── Public VFS API (mirrors MemFs exactly) ────────────────────────────────

    pub async fn write(&self, path: &VfsPath, content: Vec<u8>) -> Result<(), AvixError> {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.write(&rel, content).await;
        }
        drop(mounts);
        self.default.write(path, content).await
    }

    pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError> {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.read(&rel).await;
        }
        drop(mounts);
        self.default.read(path).await
    }

    pub async fn delete(&self, path: &VfsPath) -> Result<(), AvixError> {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.delete(&rel).await;
        }
        drop(mounts);
        self.default.delete(path).await
    }

    pub async fn exists(&self, path: &VfsPath) -> bool {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.exists(&rel).await;
        }
        drop(mounts);
        self.default.exists(path).await
    }

    pub async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError> {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(dir.as_str(), &mounts).await {
            return provider.list(&rel).await;
        }
        drop(mounts);
        self.default.list(dir).await
    }

    /// Ensure a directory "exists" in the VFS by writing a `.keep` anchor file.
    ///
    /// The MemFS `list()` operation works by prefix scan over keys. Writing `.keep`
    /// guarantees the prefix produces at least one result, making the directory
    /// listable and discoverable. Idempotent — safe to call multiple times.
    pub async fn ensure_dir(&self, path: &VfsPath) -> Result<(), AvixError> {
        let keep_str = format!("{}/.keep", path.as_str().trim_end_matches('/'));
        let keep_path = VfsPath::parse(&keep_str)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        if !self.exists(&keep_path).await {
            self.write(&keep_path, b".keep".to_vec()).await?;
        }
        Ok(())
    }
}
