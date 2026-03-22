use crate::config::{LlmConfig, ProviderConfig};
use crate::error::AvixError;
use crate::types::Modality;
use std::collections::HashMap;

pub struct RoutingEngine {
    defaults: HashMap<String, String>,
    providers: HashMap<String, ProviderConfig>,
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

        let providers = config
            .spec
            .providers
            .iter()
            .map(|p| (p.name.clone(), p.clone()))
            .collect();

        Self {
            defaults,
            providers,
        }
    }

    pub fn resolve(
        &self,
        modality: Modality,
        explicit: Option<&str>,
    ) -> Result<&ProviderConfig, AvixError> {
        let name = if let Some(explicit_name) = explicit {
            explicit_name.to_string()
        } else {
            self.defaults
                .get(modality.as_str())
                .cloned()
                .ok_or_else(|| {
                    AvixError::ConfigParse(format!("no default provider for {}", modality.as_str()))
                })?
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

        Ok(provider)
    }
}
