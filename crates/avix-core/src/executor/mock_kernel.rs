use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::instrument;

use crate::tool_registry::ToolRegistry;

#[derive(Default)]
pub struct MockKernelHandle {
    proc_spawns: Arc<Mutex<HashSet<String>>>,
    proc_kills: Arc<Mutex<HashSet<u64>>>,
    auto_approve_rr: Arc<Mutex<bool>>,
    /// Optional real registry for `list_tools` support in tests.
    pub tool_registry: Option<Arc<ToolRegistry>>,
}

impl MockKernelHandle {
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument(skip_all)]
    pub async fn record_proc_spawn(&self, agent_name: &str) {
        self.proc_spawns.lock().await.insert(agent_name.to_string());
    }

    #[instrument(skip_all)]
    pub async fn received_proc_spawn(&self, agent_name: &str) -> bool {
        self.proc_spawns.lock().await.contains(agent_name)
    }

    #[instrument(skip_all)]
    pub async fn record_proc_kill(&self, pid: u64) {
        self.proc_kills.lock().await.insert(pid);
    }

    #[instrument(skip_all)]
    pub async fn received_proc_kill(&self, pid: u64) -> bool {
        self.proc_kills.lock().await.contains(&pid)
    }

    #[instrument(skip_all)]
    pub async fn auto_approve_resource_request(&self) {
        *self.auto_approve_rr.lock().await = true;
    }

    #[instrument(skip_all)]
    pub async fn is_auto_approve(&self) -> bool {
        *self.auto_approve_rr.lock().await
    }

    /// List tools from an optional real registry, filtered by namespace/keyword/granted_only.
    #[instrument(skip(self, token))]
    pub async fn list_tools(
        &self,
        namespace: String,
        keyword: String,
        granted_only: bool,
        token: &crate::types::token::CapabilityToken,
    ) -> Vec<serde_json::Value> {
        let summaries = if let Some(reg) = &self.tool_registry {
            reg.list_all().await
        } else {
            vec![]
        };

        summaries
            .into_iter()
            .filter(|s| {
                if granted_only && !token.has_tool(&s.name) {
                    return false;
                }
                if !namespace.is_empty() && s.namespace != namespace {
                    return false;
                }
                if !keyword.is_empty()
                    && !s.name.contains(&keyword)
                    && !s.description.contains(&keyword)
                {
                    return false;
                }
                true
            })
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "state": s.state
                })
            })
            .collect()
    }
}
