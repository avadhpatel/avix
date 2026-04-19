use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::instrument;

use crate::error::AvixError;
use crate::memfs::path::VfsPath;
use crate::tool_registry::ToolRegistry;

#[derive(Debug)]
pub struct ToolsMemFs {
    files: Arc<RwLock<std::collections::HashMap<String, Vec<u8>>>>,
}

impl ToolsMemFs {
    #[instrument]
    pub fn new() -> Self {
        Self {
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    #[instrument]
    pub async fn populate_from_registry(registry: &Arc<ToolRegistry>) -> Result<(), AvixError> {
        let tools = registry.list_all().await;
        let mut files = self.files.write().await;

        // Group tools by namespace (first path component)
        let mut by_ns: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for tool in tools {
            let ns = tool.name.split('/').next().unwrap_or("").to_string();
            by_ns.entry(ns).or_default().push(tool.name);
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
        let guard = registry.inner.read().await;
        for (name, record) in guard.iter() {
            let yaml = Self::generate_tool_yaml(&record.entry);
            files.insert(format!("/tools/{}.yaml", name.replace('/', "-")), yaml.into());
        }

        // Also add index file
        let index: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let index_yaml = serde_yaml::to_string(&index).unwrap_or_default();
        files.insert("/tools/index.yaml".to_string(), index_yaml.into());

        Ok(())
    }

    #[instrument]
    fn generate_tool_yaml(entry: &crate::tool_registry::entry::ToolEntry) -> String {
        let name = entry.name.as_str();
        let desc = &entry.descriptor;

        let description = desc.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let short = desc.get("short").and_then(|v| v.as_str()).unwrap_or(description);
        let detailed = desc.get("detailed").and_then(|v| v.as_str()).unwrap_or("");
        let handler_sig = desc.get("handler_signature").and_then(|v| v.as_str()).unwrap_or("");
        let domain = desc.get("domain").and_then(|v| v.as_str()).unwrap_or("");
        let caps = &entry.capabilities_required;

        // Determine state based on tool state
        let state = match entry.state {
            crate::types::tool::ToolState::Available => "available",
            crate::types::tool::ToolState::Unavailable => "unavailable",
            crate::types::tool::ToolState::Degraded => "degraded",
        };

        // Build YAML
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
        if !handler_sig.is_empty() {
            yaml.push_str(&format!("handler_signature: {}\n", handler_sig));
        }

        yaml
    }

    #[instrument]
    pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError> {
        self.files
            .read()
            .await
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| AvixError::NotFound(format!("ENOENT: {}", path.as_str())))
    }

    #[instrument]
    pub async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError> {
        let prefix = format!("{}/", dir.as_str().trim_end_matches('/'));
        let guard = self.files.read().await;

        let mut seen = std::collections::HashSet::new();
        for k in guard.keys().filter(|k| k.starts_with(&prefix)) {
            let rest = &k[prefix.len()..];
            let first = rest.split('/').next().unwrap_or(rest);
            if !first.is_empty() {
                seen.insert(first.to_string());
            }
        }

        if seen.is_empty() {
            return Err(AvixError::NotFound(format!("ENOENT: {}", dir.as_str())));
        }
        Ok(seen.into_iter().collect())
    }

    #[instrument]
    pub async fn exists(&self, path: &VfsPath) -> bool {
        self.files.read().await.contains_key(path.as_str())
    }
}

impl Default for ToolsMemFs {
    #[instrument]
    fn default() -> Self {
        Self::new()
    }
}