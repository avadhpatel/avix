use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

#[derive(Debug, Clone)]
pub struct OAuth2TokenState {
    pub access_token: String,
    pub refresh_token: String,
    pub expiry: DateTime<Utc>,
}

#[derive(Clone)]
pub struct RefreshConfig {
    pub provider_name: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_before_expiry_min: u32,
}

pub type TokenStore = Arc<RwLock<HashMap<String, OAuth2TokenState>>>;

#[derive(Default)]
pub struct RefreshScheduler {
    handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
}

impl RefreshScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedule a continuous refresh loop for a provider.
    /// `on_degraded` is called when refresh fails (to update routing engine status).
    pub async fn schedule<F>(
        &self,
        config: RefreshConfig,
        token_store: TokenStore,
        http_client: Arc<reqwest::Client>,
        on_degraded: F,
    ) where
        F: Fn(String) + Send + Sync + 'static,
    {
        let name = config.provider_name.clone();
        let handle = tokio::spawn(async move {
            loop {
                // Calculate time until refresh needed
                let sleep_dur = {
                    let store = token_store.read().await;
                    if let Some(state) = store.get(&config.provider_name) {
                        let now = Utc::now();
                        let refresh_at = state.expiry
                            - chrono::Duration::minutes(config.refresh_before_expiry_min as i64);
                        if refresh_at > now {
                            (refresh_at - now)
                                .to_std()
                                .unwrap_or(Duration::from_secs(60))
                        } else {
                            Duration::from_secs(0)
                        }
                    } else {
                        Duration::from_secs(60)
                    }
                };

                tokio::time::sleep(sleep_dur).await;

                // Attempt refresh
                let refresh_token = {
                    let store = token_store.read().await;
                    store
                        .get(&config.provider_name)
                        .map(|s| s.refresh_token.clone())
                };

                if let Some(rt) = refresh_token {
                    match do_refresh(&http_client, &config, &rt).await {
                        Ok(new_state) => {
                            token_store
                                .write()
                                .await
                                .insert(config.provider_name.clone(), new_state);
                            tracing::info!(provider = %config.provider_name, "OAuth2 token refreshed");
                        }
                        Err(e) => {
                            tracing::error!(provider = %config.provider_name, error = %e, "OAuth2 refresh failed");
                            on_degraded(config.provider_name.clone());
                            // Retry in 60 seconds
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                    }
                }
            }
        });
        self.handles.lock().await.insert(name, handle);
    }

    pub async fn cancel(&self, provider_name: &str) {
        if let Some(handle) = self.handles.lock().await.remove(provider_name) {
            handle.abort();
        }
    }
}

async fn do_refresh(
    client: &reqwest::Client,
    config: &RefreshConfig,
    refresh_token: &str,
) -> anyhow::Result<OAuth2TokenState> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
    ];

    let resp = client.post(&config.token_url).form(&params).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("token refresh failed: HTTP {status}: {body}");
    }

    let json: serde_json::Value = resp.json().await?;
    let access_token = json["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing access_token"))?
        .to_string();
    let new_refresh_token = json["refresh_token"]
        .as_str()
        .unwrap_or(refresh_token)
        .to_string();
    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);
    let expiry = Utc::now() + chrono::Duration::seconds(expires_in as i64);

    Ok(OAuth2TokenState {
        access_token,
        refresh_token: new_refresh_token,
        expiry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_config_fields() {
        let cfg = RefreshConfig {
            provider_name: "myp".into(),
            token_url: "https://example.com/token".into(),
            client_id: "client123".into(),
            client_secret: "secret456".into(),
            refresh_before_expiry_min: 5,
        };
        assert_eq!(cfg.provider_name, "myp");
        assert_eq!(cfg.token_url, "https://example.com/token");
        assert_eq!(cfg.client_id, "client123");
        assert_eq!(cfg.client_secret, "secret456");
        assert_eq!(cfg.refresh_before_expiry_min, 5);
    }

    #[test]
    fn test_token_state_fields() {
        let expiry = Utc::now();
        let state = OAuth2TokenState {
            access_token: "tok_abc".into(),
            refresh_token: "ref_xyz".into(),
            expiry,
        };
        assert_eq!(state.access_token, "tok_abc");
        assert_eq!(state.refresh_token, "ref_xyz");
        assert!((state.expiry - expiry).num_seconds().abs() < 1);
    }

    #[test]
    fn test_scheduler_new() {
        let sched = RefreshScheduler::new();
        // Merely constructing it should not panic
        drop(sched);
    }

    #[tokio::test]
    async fn test_cancel_noop() {
        let sched = RefreshScheduler::new();
        // Cancelling a nonexistent provider should not panic
        sched.cancel("nonexistent-provider").await;
    }

    #[test]
    fn test_token_state_expiry_in_future() {
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let state = OAuth2TokenState {
            access_token: "access123".into(),
            refresh_token: "refresh456".into(),
            expiry,
        };
        assert!(state.expiry > Utc::now(), "expiry should be in the future");
    }

    #[test]
    fn test_token_state_expiry_in_past() {
        let expiry = Utc::now() - chrono::Duration::minutes(5);
        let state = OAuth2TokenState {
            access_token: "expired-tok".into(),
            refresh_token: "ref".into(),
            expiry,
        };
        assert!(state.expiry < Utc::now(), "expiry should be in the past");
    }

    #[tokio::test]
    async fn test_token_store_insert_and_read() {
        let store: TokenStore = Arc::new(RwLock::new(HashMap::new()));
        let token = OAuth2TokenState {
            access_token: "tok_abc".into(),
            refresh_token: "ref_xyz".into(),
            expiry: Utc::now() + chrono::Duration::hours(24),
        };
        store
            .write()
            .await
            .insert("test-provider".into(), token.clone());

        let guard = store.read().await;
        let retrieved = guard.get("test-provider").unwrap();
        assert_eq!(retrieved.access_token, "tok_abc");
        assert_eq!(retrieved.refresh_token, "ref_xyz");
    }

    #[tokio::test]
    async fn test_scheduler_schedule_and_cancel_immediately() {
        let scheduler = RefreshScheduler::new();
        let store: TokenStore = Arc::new(RwLock::new(HashMap::new()));
        // Add a token with far-future expiry so the scheduled task sleeps a long time
        store.write().await.insert(
            "test-provider".into(),
            OAuth2TokenState {
                access_token: "tok".into(),
                refresh_token: "ref".into(),
                expiry: Utc::now() + chrono::Duration::hours(24),
            },
        );

        let config = RefreshConfig {
            provider_name: "test-provider".into(),
            token_url: "http://localhost:19999/token".into(), // unreachable — won't be called
            client_id: "client".into(),
            client_secret: "secret".into(),
            refresh_before_expiry_min: 5,
        };

        let http_client = Arc::new(reqwest::Client::new());
        scheduler
            .schedule(config, Arc::clone(&store), http_client, |_| {})
            .await;
        // Immediately cancel — no HTTP call should occur
        scheduler.cancel("test-provider").await;
    }

    #[tokio::test]
    async fn test_scheduler_multiple_providers_schedule_cancel() {
        let scheduler = RefreshScheduler::new();
        let http_client = Arc::new(reqwest::Client::new());

        for name in &["provider-a", "provider-b"] {
            let store: TokenStore = Arc::new(RwLock::new(HashMap::new()));
            store.write().await.insert(
                name.to_string(),
                OAuth2TokenState {
                    access_token: "tok".into(),
                    refresh_token: "ref".into(),
                    expiry: Utc::now() + chrono::Duration::hours(24),
                },
            );
            let config = RefreshConfig {
                provider_name: name.to_string(),
                token_url: "http://localhost:19999/token".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                refresh_before_expiry_min: 5,
            };
            scheduler
                .schedule(config, store, Arc::clone(&http_client), |_| {})
                .await;
        }

        scheduler.cancel("provider-a").await;
        scheduler.cancel("provider-b").await;
        // No panic = success
    }

    #[tokio::test]
    async fn test_scheduler_no_token_in_store() {
        // If no token in store, the loop sleeps 60s and then tries refresh
        // We test that scheduling without a token in the store doesn't panic on cancel
        let scheduler = RefreshScheduler::new();
        let store: TokenStore = Arc::new(RwLock::new(HashMap::new()));
        // empty store

        let config = RefreshConfig {
            provider_name: "empty-provider".into(),
            token_url: "http://localhost:19999/token".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            refresh_before_expiry_min: 5,
        };
        let http_client = Arc::new(reqwest::Client::new());
        scheduler.schedule(config, store, http_client, |_| {}).await;
        scheduler.cancel("empty-provider").await;
    }
}
