use std::sync::Arc;
use uuid::Uuid;

use crate::error::AvixError;
use crate::session::{PersistentSessionStore, SessionRecord};

pub struct SessionManager {
    store: Option<Arc<PersistentSessionStore>>,
}

impl SessionManager {
    pub fn new(store: Option<Arc<PersistentSessionStore>>) -> Self {
        Self { store }
    }

    pub fn with_store(mut self, store: Arc<PersistentSessionStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub async fn create_session(
        &self,
        username: &str,
        origin_agent: &str,
        title: &str,
        goal: &str,
        owner_pid: u32,
    ) -> Result<SessionRecord, AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("session store not configured".into()))?;
        let record = SessionRecord::new(
            Uuid::new_v4(),
            username.to_string(),
            origin_agent.to_string(),
            title.to_string(),
            goal.to_string(),
            owner_pid,
        );
        store.create(&record).await?;
        Ok(record)
    }

    pub async fn list_sessions(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        match &self.store {
            Some(s) => s.list_for_user(username).await,
            None => Ok(vec![]),
        }
    }

    pub async fn get_session(&self, session_id: &Uuid) -> Result<Option<SessionRecord>, AvixError> {
        match &self.store {
            Some(s) => s.get(session_id).await,
            None => Ok(None),
        }
    }

    pub async fn update_session(&self, session: &SessionRecord) -> Result<(), AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("session store not configured".into()))?;
        store.update(session).await
    }

    pub fn store(&self) -> Option<&Arc<PersistentSessionStore>> {
        self.store.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn create_and_list_session() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        );
        let manager = SessionManager::new(Some(store));

        let session = manager
            .create_session("alice", "agent", "Test Session", "test goal", 42)
            .await
            .unwrap();

        let sessions = manager.list_sessions("alice").await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, session.id);
    }

    #[tokio::test]
    async fn get_session_returns_none_when_not_found() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        );
        let manager = SessionManager::new(Some(store));

        let result = manager.get_session(&uuid::Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_sessions_returns_empty_when_no_store() {
        let manager = SessionManager::new(None);
        let result = manager.list_sessions("alice").await.unwrap();
        assert!(result.is_empty());
    }
}