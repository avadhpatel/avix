use std::sync::Arc;
use uuid::Uuid;

use tracing::{debug, info, warn};
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
        info!(session_id = %record.id, owner_pid, "created session");
        Ok(record)
    }

    pub async fn list_sessions(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        debug!(username, "listing sessions for user");
        match &self.store {
            Some(s) => {
                let sessions = s.list_for_user(username).await?;
                debug!(username, count = sessions.len(), "listed sessions");
                Ok(sessions)
            }
            None => Ok(vec![]),
        }
    }

    pub async fn get_session(&self, session_id: &Uuid) -> Result<Option<SessionRecord>, AvixError> {
        debug!(session_id = %session_id, "getting session");
        match &self.store {
            Some(s) => {
                let session = s.get(session_id).await?;
                if session.is_some() {
                    debug!(session_id = %session_id, "found session");
                } else {
                    debug!(session_id = %session_id, "session not found");
                }
                Ok(session)
            }
            None => Ok(None),
        }
    }

    pub async fn update_session(&self, session: &SessionRecord) -> Result<(), AvixError> {
        let store = self.store.as_ref()
            .ok_or_else(|| AvixError::NotFound("session store not configured".into()))?;
        if let Err(e) = store.update(session).await {
            warn!(session_id = %session.id, error = %e, "failed to update session");
            return Err(e);
        }
        debug!(session_id = %session.id, "updated session");
        Ok(())
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