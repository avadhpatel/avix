use crate::llm_svc::routing::{ProviderStatus, RoutingEngine};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::instrument;

pub struct HealthMonitor {
    routing: Arc<RoutingEngine>,
    http_client: Arc<reqwest::Client>,
}

pub struct HealthCheckSpec {
    pub provider_name: String,
    pub base_url: String,
    pub endpoint: String,
    pub interval_sec: u64,
}

impl HealthMonitor {
    pub fn new(routing: Arc<RoutingEngine>, http_client: Arc<reqwest::Client>) -> Self {
        Self {
            routing,
            http_client,
        }
    }

    /// Start a background health check loop for one provider.
    /// Returns the JoinHandle so the caller can cancel it.
    pub fn start_provider(&self, spec: HealthCheckSpec) -> JoinHandle<()> {
        let routing = Arc::clone(&self.routing);
        let client = Arc::clone(&self.http_client);
        let interval = Duration::from_secs(spec.interval_sec);

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let url = format!("{}{}", spec.base_url.trim_end_matches('/'), spec.endpoint);
                match client
                    .get(&url)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        routing
                            .update_status(&spec.provider_name, ProviderStatus::Available)
                            .await;
                        tracing::debug!(provider = %spec.provider_name, "health check OK");
                    }
                    Ok(resp) => {
                        let reason = format!("HTTP {}", resp.status());
                        routing
                            .update_status(&spec.provider_name, ProviderStatus::Degraded { reason })
                            .await;
                        tracing::warn!(provider = %spec.provider_name, "health check degraded");
                    }
                    Err(e) => {
                        routing
                            .update_status(
                                &spec.provider_name,
                                ProviderStatus::Unavailable {
                                    reason: e.to_string(),
                                },
                            )
                            .await;
                        tracing::error!(
                            provider = %spec.provider_name,
                            error = %e,
                            "health check failed"
                        );
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmConfig;
    use crate::llm_svc::routing::RoutingEngine;

    fn make_routing() -> Arc<RoutingEngine> {
        let config = LlmConfig::from_str(
            r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: anthropic
    speech: anthropic
    transcription: anthropic
    embedding: anthropic
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: ANTHROPIC_API_KEY
        header: x-api-key
"#,
        )
        .unwrap();
        Arc::new(RoutingEngine::from_config(&config))
    }

    #[test]
    fn test_health_monitor_new() {
        let routing = make_routing();
        let client = Arc::new(reqwest::Client::new());
        let _monitor = HealthMonitor::new(routing, client);
    }

    #[test]
    fn test_health_check_spec_fields() {
        let spec = HealthCheckSpec {
            provider_name: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            endpoint: "/v1/models".into(),
            interval_sec: 30,
        };
        assert_eq!(spec.provider_name, "anthropic");
        assert_eq!(spec.base_url, "https://api.anthropic.com");
        assert_eq!(spec.endpoint, "/v1/models");
        assert_eq!(spec.interval_sec, 30);
    }

    #[tokio::test]
    async fn test_health_monitor_start_and_abort() {
        let routing = make_routing();
        let client = Arc::new(reqwest::Client::new());
        let monitor = HealthMonitor::new(routing, client);

        let spec = HealthCheckSpec {
            provider_name: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            endpoint: "/v1/models".into(),
            interval_sec: 3600, // very long — first sleep won't complete
        };

        let handle = monitor.start_provider(spec);
        // Immediately abort the background task
        handle.abort();
        // No panic = success
    }

    #[tokio::test]
    async fn test_health_monitor_multiple_providers() {
        let routing = make_routing();
        let client = Arc::new(reqwest::Client::new());
        let monitor = HealthMonitor::new(Arc::clone(&routing), Arc::clone(&client));

        let spec1 = HealthCheckSpec {
            provider_name: "provider-1".into(),
            base_url: "http://localhost:19998".into(),
            endpoint: "/health".into(),
            interval_sec: 3600,
        };
        let spec2 = HealthCheckSpec {
            provider_name: "provider-2".into(),
            base_url: "http://localhost:19997".into(),
            endpoint: "/health".into(),
            interval_sec: 3600,
        };

        let h1 = monitor.start_provider(spec1);
        let h2 = monitor.start_provider(spec2);
        h1.abort();
        h2.abort();
    }

    #[test]
    fn test_health_monitor_fields() {
        let routing = make_routing();
        let client = Arc::new(reqwest::Client::new());
        let _monitor = HealthMonitor::new(Arc::clone(&routing), Arc::clone(&client));
        // Just verifying construction with different routing instances
        let _monitor2 = HealthMonitor::new(routing, client);
    }
}
