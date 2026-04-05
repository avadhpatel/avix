use std::sync::Arc;
use tokio::sync::RwLock;

use super::context::{VfsCallerContext, VfsPermissions};
use super::local_provider::LocalProvider;
use super::path::VfsPath;
use super::vfs::MemFs;
use crate::error::AvixError;

pub struct VfsRouter {
    /// Sorted descending by prefix length so the longest match is found first.
    mounts: RwLock<Vec<(String, LocalProvider)>>,
    /// In-memory mount points (for /tools, etc.)
    mem_mounts: RwLock<Vec<(String, Arc<MemFs>)>>,
    /// Tool registry reference for /tools/ population
    tool_registry: RwLock<Option<Arc<crate::tool_registry::ToolRegistry>>>,
    /// Permissions store reference
    permissions_store: RwLock<Option<Arc<crate::tool_registry::ToolPermissionsStore>>>,
    /// Current caller context (set per-request)
    caller: RwLock<Option<VfsCallerContext>>,
    default: MemFs,
}

impl std::fmt::Debug for VfsRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsRouter")
            .field("mounts", &"...")
            .field("mem_mounts", &"...")
            .finish()
    }
}

impl Default for VfsRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for VfsRouter {
    fn clone(&self) -> Self {
        Self {
            mounts: RwLock::new(Vec::new()),
            mem_mounts: RwLock::new(Vec::new()),
            tool_registry: RwLock::new(None),
            permissions_store: RwLock::new(None),
            caller: RwLock::new(None),
            default: MemFs::new(),
        }
    }
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            mounts: RwLock::new(Vec::new()),
            mem_mounts: RwLock::new(Vec::new()),
            tool_registry: RwLock::new(None),
            permissions_store: RwLock::new(None),
            caller: RwLock::new(None),
            default: MemFs::new(),
        }
    }

    /// Set the tool registry for /tools/ VFS population
    pub async fn set_tool_registry(&self, registry: Arc<crate::tool_registry::ToolRegistry>) {
        let mut tr = self.tool_registry.write().await;
        *tr = Some(registry);
    }

    /// Set the permissions store for access control
    pub async fn set_permissions_store(
        &self,
        store: Arc<crate::tool_registry::ToolPermissionsStore>,
    ) {
        let mut ps = self.permissions_store.write().await;
        *ps = Some(store);
    }

    /// Set the caller context for the current request (for access control)
    pub async fn set_caller(&self, caller: Option<VfsCallerContext>) {
        let mut c = self.caller.write().await;
        *c = caller;
    }

    /// Get current caller context
    pub async fn caller(&self) -> Option<VfsCallerContext> {
        let c = self.caller.read().await;
        c.clone()
    }

    /// Check if caller has permission to access a path
    pub async fn check_access(&self, path: &VfsPath, required: &str) -> Result<(), AvixError> {
        let caller = self.caller.read().await;
        let caller = match caller.as_ref() {
            Some(c) => c,
            None => return Err(AvixError::CapabilityDenied("no caller context".to_string())),
        };

        let perms = VfsPermissions::for_path(path.as_str());

        match required {
            "r" if !perms.can_read(caller) => Err(AvixError::CapabilityDenied(format!(
                "no read permission on {}",
                path.as_str()
            ))),
            "w" if !perms.can_write(caller) => Err(AvixError::CapabilityDenied(format!(
                "no write permission on {}",
                path.as_str()
            ))),
            "x" if !perms.can_execute(caller) => Err(AvixError::CapabilityDenied(format!(
                "no execute permission on {}",
                path.as_str()
            ))),
            _ => Ok(()),
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

    /// Add an in-memory filesystem for a prefix (for /tools, etc.)
    pub async fn mount_memfs(&self, prefix: String, fs: Arc<MemFs>) {
        let prefix = prefix.trim_end_matches('/').to_string();
        let mut mem_mounts = self.mem_mounts.write().await;
        mem_mounts.retain(|(p, _)| p != &prefix);
        mem_mounts.push((prefix, fs));
        mem_mounts.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()));
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
        // Check write permission if caller context is set
        if let Some(caller) = self.caller.read().await.as_ref() {
            let perms = VfsPermissions::for_path(path.as_str());
            if !perms.can_write(caller) {
                return Err(AvixError::CapabilityDenied(format!(
                    "permission denied: cannot write {} (effective: {})",
                    path.as_str(),
                    perms.effective_for(caller)
                )));
            }
        }

        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.write(&rel, content).await;
        }
        drop(mounts);
        // Check in-memory mounts
        let mem_mounts = self.mem_mounts.read().await;
        for (prefix, fs) in mem_mounts.iter() {
            if path.as_str() == prefix || path.as_str().starts_with(&format!("{prefix}/")) {
                return fs.write(path, content).await;
            }
        }
        drop(mem_mounts);
        self.default.write(path, content).await
    }

    pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError> {
        // Check read permission if caller context is set
        if let Some(caller) = self.caller.read().await.as_ref() {
            let perms = VfsPermissions::for_path(path.as_str());
            if !perms.can_read(caller) {
                return Err(AvixError::CapabilityDenied(format!(
                    "permission denied: cannot read {} (effective: {})",
                    path.as_str(),
                    perms.effective_for(caller)
                )));
            }
        }

        // Special handling for /tools/ paths - lazy population from registry
        if path.as_str().starts_with("/tools") {
            let needs_population = {
                let mem_mounts = self.mem_mounts.read().await;
                let mut found = false;
                for (prefix, fs) in mem_mounts.iter() {
                    if *prefix == "tools" {
                        // Check if already populated by looking for index file
                        let files = fs.files.read().await;
                        found = files.contains_key("/tools/index.yaml");
                        break;
                    }
                }
                !found
            };

            if needs_population {
                if let Some(registry) = self.tool_registry.read().await.as_ref() {
                    // Need to populate - drop read guard first, then get write guard
                    let fs = {
                        let mem_mounts = self.mem_mounts.read().await;
                        mem_mounts
                            .iter()
                            .find(|(p, _)| *p == "tools")
                            .map(|(_, f)| Arc::clone(f))
                    };
                    if let Some(fs) = fs {
                        Self::populate_tools_memfs(&fs, registry).await?;
                    }
                }
            }
        }

        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.read(&rel).await;
        }
        drop(mounts);
        // Check in-memory mounts
        let mem_mounts = self.mem_mounts.read().await;
        for (prefix, fs) in mem_mounts.iter() {
            if path.as_str() == prefix || path.as_str().starts_with(&format!("{prefix}/")) {
                return fs.read(path).await;
            }
        }
        drop(mem_mounts);
        self.default.read(path).await
    }

    async fn populate_tools_memfs(
        fs: &Arc<MemFs>,
        registry: &Arc<crate::tool_registry::ToolRegistry>,
    ) -> Result<(), AvixError> {
        let tools = registry.list_all().await;
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let mut files = fs.files.write().await;

        // Group tools by namespace (first path component)
        let mut by_ns: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for name in &tool_names {
            let ns = name.split('/').next().unwrap_or("").to_string();
            by_ns.entry(ns).or_default().push(name.clone());
        }

        // Create directory index files
        for (ns, tool_names) in &by_ns {
            let dir_content = tool_names
                .iter()
                .map(|n| format!("{}.yaml", n.replace('/', "-")))
                .collect::<Vec<_>>()
                .join("\n");
            files.insert(format!("/tools/{}", ns), dir_content.into());
        }

        // Create root /tools/ directory listing
        let root_content = by_ns
            .keys()
            .map(|ns| format!("{}/", ns))
            .collect::<Vec<_>>()
            .join("\n");
        files.insert("/tools".to_string(), root_content.into());

        // For each tool, generate its YAML descriptor
        let entries = registry.get_all_entries().await;
        for entry in entries {
            let yaml = Self::generate_tool_yaml(&entry, None);
            files.insert(
                format!("/tools/{}.yaml", entry.name.as_str().replace('/', "-")),
                yaml.into(),
            );
        }

        // Also add index file
        let index_yaml = serde_yaml::to_string(&tool_names).unwrap_or_default();
        files.insert("/tools/index.yaml".to_string(), index_yaml.into());

        Ok(())
    }

    fn generate_tool_yaml(
        entry: &crate::tool_registry::entry::ToolEntry,
        caller: Option<&VfsCallerContext>,
    ) -> String {
        use crate::types::tool::ToolState;

        let name = entry.name.as_str();
        let desc = &entry.descriptor;

        let description = desc
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let short = desc
            .get("short")
            .and_then(|v| v.as_str())
            .unwrap_or(description);
        let detailed = desc.get("detailed").and_then(|v| v.as_str()).unwrap_or("");
        let handler_sig = desc
            .get("handler_signature")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let domain = desc.get("domain").and_then(|v| v.as_str()).unwrap_or("");
        let caps = &entry.capabilities_required;

        // Determine if tool is available for this caller
        let state = if let Some(c) = caller {
            if let Some(token) = &c.token {
                let has_caps = caps.iter().all(|cap| token.has_tool(cap));
                if has_caps {
                    "available"
                } else {
                    "unavailable"
                }
            } else if c.is_admin {
                "available"
            } else {
                "unavailable"
            }
        } else {
            match entry.state {
                ToolState::Available => "available",
                ToolState::Unavailable => "unavailable",
                ToolState::Degraded => "degraded",
            }
        };

        let mut yaml = String::new();
        yaml.push_str(&format!("name: {}\n", name));
        yaml.push_str(&format!("description: {}\n", description));
        yaml.push_str(&format!("short: {}\n", short));
        yaml.push_str("detailed: |\n");
        for line in detailed.lines() {
            yaml.push_str(&format!("  {}\n", line));
        }
        yaml.push_str(&format!("domain: {}\n", domain));
        yaml.push_str("capabilities_required:\n");
        for cap in caps {
            yaml.push_str(&format!("  - {}\n", cap));
        }
        yaml.push_str(&format!("state: {}\n", state));
        yaml.push_str(&format!("owner: {}\n", entry.owner));

        // Add permissions from tool entry
        yaml.push_str("permissions:\n");
        yaml.push_str(&format!("  owner: {}\n", entry.permissions.owner));
        yaml.push_str(&format!(
            "  crew: {}\n",
            if entry.permissions.crew.is_empty() {
                "---"
            } else {
                &entry.permissions.crew
            }
        ));
        yaml.push_str(&format!("  all: {}\n", entry.permissions.all));

        // Add request_access for unavailable tools
        if state == "unavailable" && !caps.is_empty() {
            yaml.push_str("request_access: cap/request-tool\n");
        }

        if !handler_sig.is_empty() {
            yaml.push_str(&format!("handler_signature: {}\n", handler_sig));
        }

        yaml
    }

    pub async fn delete(&self, path: &VfsPath) -> Result<(), AvixError> {
        // Check write permission for delete
        if let Some(caller) = self.caller.read().await.as_ref() {
            let perms = VfsPermissions::for_path(path.as_str());
            if !perms.can_write(caller) {
                return Err(AvixError::CapabilityDenied(format!(
                    "permission denied: cannot delete {} (effective: {})",
                    path.as_str(),
                    perms.effective_for(caller)
                )));
            }
        }

        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            return provider.delete(&rel).await;
        }
        drop(mounts);
        // Check in-memory mounts
        let mem_mounts = self.mem_mounts.read().await;
        for (prefix, fs) in mem_mounts.iter() {
            if path.as_str() == prefix || path.as_str().starts_with(&format!("{prefix}/")) {
                return fs.delete(path).await;
            }
        }
        drop(mem_mounts);
        self.default.delete(path).await
    }

    pub async fn exists(&self, path: &VfsPath) -> bool {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(path.as_str(), &mounts).await {
            if provider.exists(&rel).await {
                return true;
            }
        }
        drop(mounts);
        // Check in-memory mounts
        let mem_mounts = self.mem_mounts.read().await;
        for (prefix, fs) in mem_mounts.iter() {
            if path.as_str() == prefix
                || path.as_str().starts_with(&format!("{prefix}/")) && fs.exists(path).await
            {
                return true;
            }
        }
        drop(mem_mounts);
        self.default.exists(path).await
    }

    pub async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError> {
        let mounts = self.mounts.read().await;
        if let Some((provider, rel)) = self.route(dir.as_str(), &mounts).await {
            return provider.list(&rel).await;
        }
        drop(mounts);
        // Check in-memory mounts
        let mem_mounts = self.mem_mounts.read().await;
        for (prefix, fs) in mem_mounts.iter() {
            if dir.as_str() == prefix || dir.as_str().starts_with(&format!("{prefix}/")) {
                return fs.list(dir).await;
            }
        }
        drop(mem_mounts);
        self.default.list(dir).await
    }

    /// Ensure a directory "exists" in the VFS by writing a `.keep` anchor file.
    ///
    /// The MemFS `list()` operation works by prefix scan over keys. Writing `.keep`
    /// guarantees the prefix produces at least one result, making the directory
    /// listable and discoverable. Idempotent — safe to call multiple times.
    pub async fn ensure_dir(&self, path: &VfsPath) -> Result<(), AvixError> {
        let keep_str = format!("{}/.keep", path.as_str().trim_end_matches('/'));
        let keep_path =
            VfsPath::parse(&keep_str).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        if !self.exists(&keep_path).await {
            self.write(&keep_path, b".keep".to_vec()).await?;
        }
        Ok(())
    }
}
