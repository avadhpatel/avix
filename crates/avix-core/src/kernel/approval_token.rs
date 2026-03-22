use crate::error::AvixError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct ApprovalTokenStore {
    tokens: Arc<RwLock<HashMap<String, Arc<AtomicBool>>>>,
}

impl ApprovalTokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create(&self, _hil_id: &str) -> String {
        let token_id = uuid::Uuid::new_v4().to_string();
        let used = Arc::new(AtomicBool::new(false));
        self.tokens.write().await.insert(token_id.clone(), used);
        token_id
    }

    pub async fn consume(&self, token_id: &str) -> Result<(), AvixError> {
        let guard = self.tokens.read().await;
        let used = guard
            .get(token_id)
            .ok_or_else(|| AvixError::CapabilityDenied("token not found".into()))?;
        // Atomically set true only if currently false
        used.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| AvixError::CapabilityDenied("EUSED: token already used".into()))?;
        Ok(())
    }
}
