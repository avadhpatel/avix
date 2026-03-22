use crate::error::AvixError;
use crate::types::Modality;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub modality: Modality,
    #[serde(rename = "contextWindow", default)]
    pub context_window: Option<u64>,
    pub tier: String,
    #[serde(default)]
    pub dimensions: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderLimits {
    #[serde(rename = "requestsPerMinute", default)]
    pub requests_per_minute: u32,
    #[serde(rename = "tokensPerMinute", default)]
    pub tokens_per_minute: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTimeout {
    #[serde(rename = "connectMs", default = "default_connect_ms")]
    pub connect_ms: u64,
    #[serde(rename = "readMs", default = "default_read_ms")]
    pub read_ms: u64,
}

fn default_connect_ms() -> u64 {
    3000
}
fn default_read_ms() -> u64 {
    120000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(rename = "maxAttempts", default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(rename = "backoffMs", default)]
    pub backoff_ms: u64,
    #[serde(rename = "retryOn", default)]
    pub retry_on: Vec<u16>,
}

fn default_max_attempts() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "intervalSec", default = "default_interval")]
    pub interval_sec: u64,
    pub endpoint: String,
}

fn default_true() -> bool {
    true
}
fn default_interval() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    ApiKey {
        #[serde(rename = "secretName")]
        secret_name: String,
        header: String,
        #[serde(default)]
        prefix: Option<String>,
    },
    Oauth2 {
        #[serde(rename = "secretName")]
        secret_name: String,
        #[serde(rename = "tokenUrl")]
        token_url: String,
        #[serde(rename = "clientId")]
        client_id: String,
        #[serde(rename = "clientSecretName")]
        client_secret_name: String,
        scopes: Vec<String>,
        #[serde(rename = "refreshBeforeExpiryMin")]
        refresh_before_expiry_min: u32,
    },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub modalities: Vec<Modality>,
    pub auth: ProviderAuth,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub limits: Option<ProviderLimits>,
    #[serde(default)]
    pub timeout: Option<ProviderTimeout>,
    #[serde(rename = "retryPolicy", default)]
    pub retry_policy: Option<RetryPolicy>,
    #[serde(rename = "healthCheck", default)]
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultProviders {
    pub text: String,
    pub image: String,
    pub speech: String,
    pub transcription: String,
    pub embedding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSpec {
    #[serde(rename = "defaultProviders")]
    pub default_providers: DefaultProviders,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub spec: LlmSpec,
}

impl LlmConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self =
            serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), AvixError> {
        let check = |modality: Modality, provider_name: &str| -> Result<(), AvixError> {
            let provider = self
                .spec
                .providers
                .iter()
                .find(|p| p.name == provider_name)
                .ok_or_else(|| {
                    AvixError::ConfigParse(format!(
                        "defaultProvider for {}: '{}' not found in providers list",
                        modality.as_str(),
                        provider_name
                    ))
                })?;
            if !provider.modalities.contains(&modality) {
                return Err(AvixError::ConfigParse(format!(
                    "defaultProvider for {} is '{}' but that provider does not support that modality",
                    modality.as_str(),
                    provider_name
                )));
            }
            Ok(())
        };
        let dp = &self.spec.default_providers;
        check(Modality::Text, &dp.text)?;
        check(Modality::Image, &dp.image)?;
        check(Modality::Speech, &dp.speech)?;
        check(Modality::Transcription, &dp.transcription)?;
        check(Modality::Embedding, &dp.embedding)?;
        Ok(())
    }

    pub fn default_provider_for(&self, modality: Modality) -> Option<&ProviderConfig> {
        let name = match modality {
            Modality::Text => &self.spec.default_providers.text,
            Modality::Image => &self.spec.default_providers.image,
            Modality::Speech => &self.spec.default_providers.speech,
            Modality::Transcription => &self.spec.default_providers.transcription,
            Modality::Embedding => &self.spec.default_providers.embedding,
        };
        self.spec.providers.iter().find(|p| &p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_provider_yaml() -> &'static str {
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
"#
    }

    #[test]
    fn regression_two_provider_yaml_parses() {
        let cfg = LlmConfig::from_str(two_provider_yaml()).unwrap();
        assert_eq!(cfg.spec.providers.len(), 2);
        assert_eq!(cfg.spec.default_providers.text, "anthropic");
    }

    #[test]
    fn provider_with_all_new_fields_parses() {
        let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: openai
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com/v1
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: OPENAI_API_KEY
        header: Authorization
        prefix: "Bearer "
      models:
        - id: gpt-4o
          modality: text
          contextWindow: 128000
          tier: premium
        - id: text-embedding-3-small
          modality: embedding
          tier: standard
          dimensions: 1536
      limits:
        requestsPerMinute: 3000
        tokensPerMinute: 1000000
      timeout:
        connectMs: 5000
        readMs: 60000
      retryPolicy:
        maxAttempts: 5
        backoffMs: 500
        retryOn: [429, 500, 502, 503]
      healthCheck:
        enabled: true
        intervalSec: 30
        endpoint: /v1/models
"#;
        let cfg = LlmConfig::from_str(yaml).unwrap();
        let openai = &cfg.spec.providers[0];

        assert_eq!(openai.models.len(), 2);
        assert_eq!(openai.models[0].id, "gpt-4o");
        assert_eq!(openai.models[1].dimensions, Some(1536));

        let limits = openai.limits.as_ref().unwrap();
        assert_eq!(limits.requests_per_minute, 3000);
        assert_eq!(limits.tokens_per_minute, 1_000_000);

        let timeout = openai.timeout.as_ref().unwrap();
        assert_eq!(timeout.connect_ms, 5000);
        assert_eq!(timeout.read_ms, 60000);

        let retry = openai.retry_policy.as_ref().unwrap();
        assert_eq!(retry.max_attempts, 5);
        assert_eq!(retry.backoff_ms, 500);
        assert!(retry.retry_on.contains(&429));

        let hc = openai.health_check.as_ref().unwrap();
        assert!(hc.enabled);
        assert_eq!(hc.interval_sec, 30);
        assert_eq!(hc.endpoint, "/v1/models");
    }

    #[test]
    fn api_key_prefix_field_parses() {
        let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: openai
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com/v1
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: OPENAI_API_KEY
        header: Authorization
        prefix: "Bearer "
"#;
        let cfg = LlmConfig::from_str(yaml).unwrap();
        let auth = &cfg.spec.providers[0].auth;
        match auth {
            ProviderAuth::ApiKey { prefix, .. } => {
                assert_eq!(prefix.as_deref(), Some("Bearer "));
            }
            _ => panic!("expected ApiKey auth"),
        }
    }

    #[test]
    fn api_key_without_prefix_defaults_to_none() {
        let yaml = r#"
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
"#;
        let cfg = LlmConfig::from_str(yaml).unwrap();
        let auth = &cfg.spec.providers[0].auth;
        match auth {
            ProviderAuth::ApiKey { prefix, .. } => {
                assert!(prefix.is_none());
            }
            _ => panic!("expected ApiKey auth"),
        }
    }
}
