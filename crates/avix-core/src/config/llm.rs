use crate::error::AvixError;
use crate::types::Modality;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    ApiKey {
        #[serde(rename = "secretName")]
        secret_name: String,
        header: String,
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
