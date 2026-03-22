use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::session::SessionEntry;
use super::validate::validate_credential;
use crate::config::AuthConfig;
use crate::error::AvixError;

#[derive(Debug, Clone)]
pub struct AuthService {
    config: AuthConfig,
    sessions: Arc<RwLock<HashMap<String, SessionEntry>>>,
    ttl: Duration,
}

impl AuthService {
    pub fn new(config: AuthConfig) -> Self {
        Self::new_with_ttl(config, Duration::from_secs(3600))
    }

    pub fn new_with_ttl(config: AuthConfig, ttl: Duration) -> Self {
        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    pub async fn login(
        &self,
        identity_name: &str,
        credential: &str,
    ) -> Result<SessionEntry, AvixError> {
        let identity = self
            .config
            .identities
            .iter()
            .find(|i| i.name == identity_name)
            .ok_or_else(|| {
                AvixError::CapabilityDenied(format!("unknown identity: {identity_name}"))
            })?;

        if !validate_credential(&identity.credential, credential) {
            return Err(AvixError::CapabilityDenied("invalid credential".into()));
        }

        let session_id = Uuid::new_v4().to_string();
        let entry = SessionEntry {
            session_id: session_id.clone(),
            identity_name: identity.name.clone(),
            role: identity.role,
            created_at: std::time::Instant::now(),
            ttl: self.ttl,
        };

        self.sessions
            .write()
            .await
            .insert(session_id, entry.clone());
        Ok(entry)
    }

    pub async fn validate_session(&self, session_id: &str) -> Result<SessionEntry, AvixError> {
        let guard = self.sessions.read().await;
        let entry = guard
            .get(session_id)
            .ok_or_else(|| AvixError::CapabilityDenied("invalid session".into()))?;
        if entry.is_expired() {
            return Err(AvixError::CapabilityDenied("session expired".into()));
        }
        Ok(entry.clone())
    }

    pub async fn revoke_session(&self, session_id: &str) -> Result<(), AvixError> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| AvixError::CapabilityDenied("session not found".into()))?;
        Ok(())
    }

    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }
}
