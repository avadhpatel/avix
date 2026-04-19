use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

use tracing::instrument;

#[derive(Debug, Default)]
pub struct ServiceRegistry {
    services: Arc<RwLock<HashMap<String, String>>>,
    tools: Arc<RwLock<HashMap<String, String>>>,
    /// Services that require `_caller` injection on every tool call.
    caller_scoped: Arc<RwLock<HashSet<String>>>,
}

impl ServiceRegistry {
    #[instrument]
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument]
    pub async fn register(&self, name: &str, endpoint: &str) {
        self.services
            .write()
            .await
            .insert(name.to_string(), endpoint.to_string());
    }

    /// Register with an explicit `caller_scoped` flag.
    #[instrument]
    pub async fn register_with_meta(&self, name: &str, endpoint: &str, caller_scoped: bool) {
        self.services
            .write()
            .await
            .insert(name.to_string(), endpoint.to_string());
        if caller_scoped {
            self.caller_scoped.write().await.insert(name.to_string());
        }
    }

    #[instrument]
    pub async fn deregister(&self, name: &str) {
        self.services.write().await.remove(name);
        self.caller_scoped.write().await.remove(name);
    }

    #[instrument]
    pub async fn is_caller_scoped(&self, name: &str) -> bool {
        self.caller_scoped.read().await.contains(name)
    }

    #[instrument]
    pub async fn lookup(&self, name: &str) -> Option<String> {
        self.services.read().await.get(name).cloned()
    }

    #[instrument]
    pub async fn register_tool(&self, tool: &str, service: &str) {
        self.tools
            .write()
            .await
            .insert(tool.to_string(), service.to_string());
    }

    #[instrument]
    pub async fn service_for_tool(&self, tool: &str) -> Option<String> {
        self.tools.read().await.get(tool).cloned()
    }

    #[instrument]
    pub async fn tool_count(&self) -> usize {
        self.tools.read().await.len()
    }
}
