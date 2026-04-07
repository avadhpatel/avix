use std::sync::Arc;

use crate::error::AvixError;
use crate::invocation::{InvocationRecord, InvocationStatus, InvocationStore};

pub struct InvocationManager {
    store: Option<Arc<InvocationStore>>,
}

impl InvocationManager {
    pub fn new(store: Option<Arc<InvocationStore>>) -> Self {
        Self { store }
    }

    pub fn with_store(mut self, store: Arc<InvocationStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub async fn list_invocations(
        &self,
        username: &str,
        agent_name: Option<&str>,
        live: bool,
    ) -> Result<Vec<InvocationRecord>, AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("invocation store not configured".into()))?;
        let records = match agent_name {
            Some(name) => store.list_for_agent(username, name).await?,
            None => store.list_for_user(username).await?,
        };
        if live {
            Ok(records)
        } else {
            Ok(records
                .into_iter()
                .filter(|r| {
                    !matches!(
                        r.status,
                        InvocationStatus::Running
                            | InvocationStatus::Idle
                            | InvocationStatus::Paused
                    )
                })
                .collect())
        }
    }

    pub async fn get_invocation(
        &self,
        invocation_id: &str,
    ) -> Result<Option<InvocationRecord>, AvixError> {
        match &self.store {
            Some(s) => s.get(invocation_id).await,
            None => Ok(None),
        }
    }

    pub async fn update_status(&self, invocation_id: &str, status: InvocationStatus) -> Result<(), AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("invocation store not configured".into()))?;
        store.update_status(invocation_id, status).await
    }

    pub async fn snapshot_invocation(&self, id: &str) -> Result<InvocationRecord, AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("invocation store not configured".into()))?;

        let record = store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {id} not found")))?;

        if !matches!(
            record.status,
            InvocationStatus::Running | InvocationStatus::Idle | InvocationStatus::Paused
        ) {
            return Err(AvixError::InvalidInput(
                "cannot snapshot a finalized invocation".into(),
            ));
        }

        store
            .persist_interim(id, &[], record.tokens_consumed, record.tool_calls_total)
            .await?;

        store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {id} not found")))
    }

    pub fn store(&self) -> Option<&Arc<InvocationStore>> {
        self.store.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn list_invocations_fails_when_no_store() {
        let manager = InvocationManager::new(None);
        let result = manager.list_invocations("alice", None, true).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_invocation_returns_none_when_no_store() {
        let manager = InvocationManager::new(None);
        let result = manager.get_invocation("some-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn snapshot_invocation_fails_when_not_configured() {
        let manager = InvocationManager::new(None);
        let result = manager.snapshot_invocation("some-id").await;
        assert!(result.is_err());
    }
}