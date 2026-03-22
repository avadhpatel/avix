use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Default)]
pub struct ServiceRegistry {
    services: Arc<RwLock<HashMap<String, String>>>,
    tools: Arc<RwLock<HashMap<String, String>>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(&self, name: &str, endpoint: &str) {
        self.services
            .write()
            .await
            .insert(name.to_string(), endpoint.to_string());
    }

    pub async fn deregister(&self, name: &str) {
        self.services.write().await.remove(name);
    }

    pub async fn lookup(&self, name: &str) -> Option<String> {
        self.services.read().await.get(name).cloned()
    }

    pub async fn register_tool(&self, tool: &str, service: &str) {
        self.tools
            .write()
            .await
            .insert(tool.to_string(), service.to_string());
    }

    pub async fn service_for_tool(&self, tool: &str) -> Option<String> {
        self.tools.read().await.get(tool).cloned()
    }

    pub async fn tool_count(&self) -> usize {
        self.tools.read().await.len()
    }
}
