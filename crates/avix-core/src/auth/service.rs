use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::instrument;
use uuid::Uuid;

use super::atp_token::{ATPTokenClaims, ATPTokenStore};
use super::session::{SessionEntry, SessionState};
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

    #[instrument(skip(self, credential))]
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
        let now = Utc::now();
        let entry = SessionEntry {
            session_id: session_id.clone(),
            identity_name: identity.name.clone(),
            uid: identity.uid,
            role: identity.role,
            crews: vec![],
            scope: vec![
                "proc".into(),
                "signal".into(),
                "fs".into(),
                "snap".into(),
                "cron".into(),
            ],
            state: SessionState::Active,
            connected_at: now,
            last_activity_at: now,
            idle_since: None,
            closed_at: None,
            closed_reason: None,
            agents: vec![],
            ttl: self.ttl,
        };

        self.sessions
            .write()
            .await
            .insert(session_id, entry.clone());
        Ok(entry)
    }

    #[instrument(skip(self))]
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

    #[instrument(skip(self))]
    pub async fn revoke_session(&self, session_id: &str) -> Result<(), AvixError> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| AvixError::CapabilityDenied("session not found".into()))?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Validate the current token and issue a fresh one with a new expiry.
    /// Returns `(new_token_string, new_claims)`.
    #[instrument(skip(self, old_token, token_store))]
    pub async fn refresh_token(
        &self,
        old_token: &str,
        token_store: &ATPTokenStore,
    ) -> Result<(String, ATPTokenClaims), AvixError> {
        let claims = token_store.validate(old_token).await?;
        // Session must still exist and not be expired
        self.validate_session(&claims.session_id).await?;
        let now = Utc::now();
        let new_claims = ATPTokenClaims {
            iat: now,
            exp: now + chrono::Duration::hours(8),
            ..claims
        };
        let new_token = token_store.issue(new_claims.clone()).await?;
        Ok((new_token, new_claims))
    }
}
