use crate::config::{LlmConfig, ProviderConfig};
use crate::error::AvixError;
use crate::types::Modality;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::instrument;

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    Available,
    Degraded { reason: String },
    Unavailable { reason: String },
}

pub struct RoutingEngine {
    defaults: HashMap<String, String>,
    providers: HashMap<String, ProviderConfig>,
    status: Arc<RwLock<HashMap<String, ProviderStatus>>>,
    fallback_text: Option<String>,
}

impl RoutingEngine {
    pub fn from_config(config: &LlmConfig) -> Self {
        let mut defaults = HashMap::new();
        defaults.insert("text".into(), config.spec.default_providers.text.clone());
        defaults.insert("image".into(), config.spec.default_providers.image.clone());
        defaults.insert(
            "speech".into(),
            config.spec.default_providers.speech.clone(),
        );
        defaults.insert(
            "transcription".into(),
            config.spec.default_providers.transcription.clone(),
        );
        defaults.insert(
            "embedding".into(),
            config.spec.default_providers.embedding.clone(),
        );

        let mut status_map = HashMap::new();
        let providers: HashMap<String, ProviderConfig> = config
            .spec
            .providers
            .iter()
            .map(|p| {
                status_map.insert(p.name.clone(), ProviderStatus::Available);
                (p.name.clone(), p.clone())
            })
            .collect();

        Self {
            defaults,
            providers,
            status: Arc::new(RwLock::new(status_map)),
            fallback_text: None,
        }
    }

    pub fn with_text_fallback(mut self, fallback: impl Into<String>) -> Self {
        self.fallback_text = Some(fallback.into());
        self
    }

    #[instrument(skip(self))]
    pub async fn update_status(&self, provider: &str, status: ProviderStatus) {
        self.status
            .write()
            .await
            .insert(provider.to_string(), status);
    }

    /// 4-step resolution per spec:
    /// 1. Use explicit provider name if given, else use default for modality
    /// 2. Verify provider exists
    /// 3. Verify provider supports the modality
    /// 4. Check provider health status — if Unavailable, try text fallback
    #[instrument(skip(self))]
    pub async fn resolve(
        &self,
        modality: Modality,
        explicit: Option<&str>,
    ) -> Result<&ProviderConfig, AvixError> {
        let name = if let Some(n) = explicit {
            n.to_string()
        } else {
            self.defaults
                .get(modality.as_str())
                .cloned()
                .ok_or_else(|| AvixError::NoProviderAvailable(modality.as_str().to_string()))?
        };

        let provider = self
            .providers
            .get(&name)
            .ok_or_else(|| AvixError::ConfigParse(format!("provider not found: {name}")))?;

        if !provider.modalities.contains(&modality) {
            return Err(AvixError::ConfigParse(format!(
                "provider '{}' does not support modality '{}'",
                name,
                modality.as_str()
            )));
        }

        // Check status
        let is_unavailable = {
            let status_map = self.status.read().await;
            matches!(
                status_map.get(&name),
                Some(ProviderStatus::Unavailable { .. })
            )
        };

        if is_unavailable {
            // For text modality, try fallback
            if modality == Modality::Text {
                if let Some(fb) = &self.fallback_text {
                    if let Some(fb_provider) = self.providers.get(fb) {
                        let fb_is_unavailable = {
                            let status_map = self.status.read().await;
                            matches!(status_map.get(fb), Some(ProviderStatus::Unavailable { .. }))
                        };
                        if !fb_is_unavailable {
                            tracing::debug!(primary = %name, fallback = %fb, "routing: primary unavailable, using fallback");
                            return Ok(fb_provider);
                        }
                    }
                }
            }
            return Err(AvixError::NoProviderAvailable(format!(
                "{name}: provider is unavailable"
            )));
        }

        tracing::debug!(provider = %name, modality = %modality.as_str(), "routing resolved");
        Ok(provider)
    }

    pub async fn all_statuses(&self) -> HashMap<String, ProviderStatus> {
        self.status.read().await.clone()
    }

    pub fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.get(name)
    }

    pub fn all_providers(&self) -> impl Iterator<Item = &ProviderConfig> {
        self.providers.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> LlmConfig {
        LlmConfig::from_str(
            r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: ANTHROPIC_API_KEY
        header: x-api-key
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: OPENAI_API_KEY
        header: Authorization
"#,
        )
        .unwrap()
    }

    fn make_config_with_text_fallback() -> LlmConfig {
        LlmConfig::from_str(
            r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: ANTHROPIC_API_KEY
        header: x-api-key
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: OPENAI_API_KEY
        header: Authorization
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn routing_available_provider_resolved() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        let provider = engine.resolve(Modality::Text, None).await.unwrap();
        assert_eq!(provider.name, "anthropic");
    }

    #[tokio::test]
    async fn routing_unavailable_returns_error() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        engine
            .update_status(
                "anthropic",
                ProviderStatus::Unavailable {
                    reason: "down".to_string(),
                },
            )
            .await;
        let err = engine.resolve(Modality::Text, None).await.unwrap_err();
        assert!(err.to_string().contains("unavailable") || err.to_string().contains("anthropic"));
    }

    #[tokio::test]
    async fn routing_degraded_still_resolves() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        engine
            .update_status(
                "anthropic",
                ProviderStatus::Degraded {
                    reason: "slow".to_string(),
                },
            )
            .await;
        // Degraded should still resolve
        let provider = engine.resolve(Modality::Text, None).await.unwrap();
        assert_eq!(provider.name, "anthropic");
    }

    #[tokio::test]
    async fn routing_fallback_used_when_primary_unavailable() {
        let config = make_config_with_text_fallback();
        let engine = RoutingEngine::from_config(&config).with_text_fallback("openai");
        engine
            .update_status(
                "anthropic",
                ProviderStatus::Unavailable {
                    reason: "down".to_string(),
                },
            )
            .await;
        let provider = engine.resolve(Modality::Text, None).await.unwrap();
        assert_eq!(provider.name, "openai");
    }

    #[tokio::test]
    async fn routing_all_statuses_returns_map() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        engine
            .update_status(
                "anthropic",
                ProviderStatus::Degraded {
                    reason: "high latency".to_string(),
                },
            )
            .await;
        let statuses = engine.all_statuses().await;
        assert!(statuses.contains_key("anthropic"));
        assert!(statuses.contains_key("openai"));
        assert!(matches!(
            statuses.get("anthropic"),
            Some(ProviderStatus::Degraded { .. })
        ));
    }

    #[tokio::test]
    async fn routing_resolves_explicit_provider() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        let provider = engine
            .resolve(Modality::Image, Some("openai"))
            .await
            .unwrap();
        assert_eq!(provider.name, "openai");
    }

    #[tokio::test]
    async fn routing_rejects_modality_mismatch() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        let err = engine
            .resolve(Modality::Image, Some("anthropic"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not support"));
    }

    #[tokio::test]
    async fn routing_rejects_unknown_provider() {
        let config = make_config();
        let engine = RoutingEngine::from_config(&config);
        let err = engine
            .resolve(Modality::Text, Some("unknown-provider"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
