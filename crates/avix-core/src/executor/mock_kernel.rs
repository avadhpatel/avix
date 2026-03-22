use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
pub struct MockKernelHandle {
    proc_spawns: Arc<Mutex<HashSet<String>>>,
    auto_approve_rr: Arc<Mutex<bool>>,
}

impl MockKernelHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn record_proc_spawn(&self, agent_name: &str) {
        self.proc_spawns.lock().await.insert(agent_name.to_string());
    }

    pub async fn received_proc_spawn(&self, agent_name: &str) -> bool {
        self.proc_spawns.lock().await.contains(agent_name)
    }

    pub async fn auto_approve_resource_request(&self) {
        *self.auto_approve_rr.lock().await = true;
    }

    pub async fn is_auto_approve(&self) -> bool {
        *self.auto_approve_rr.lock().await
    }
}
