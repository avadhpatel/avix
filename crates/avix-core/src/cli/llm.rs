use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct ProviderStatus {
    pub name: String,
    pub enabled: bool,
    pub modalities: Vec<String>,
    pub is_healthy: bool,
}

#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub provider: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

pub struct LlmCliHandler {
    /// Provider name -> enabled flag
    providers: RwLock<HashMap<String, bool>>,
    /// Default provider per modality
    defaults: RwLock<HashMap<String, String>>,
    /// Provider -> supported modalities (for validation)
    modalities: HashMap<String, Vec<String>>,
    /// Token usage counters
    usage: RwLock<HashMap<String, TokenUsage>>,
}

impl LlmCliHandler {
    pub fn new(
        provider_names: Vec<String>,
        provider_modalities: HashMap<String, Vec<String>>,
    ) -> Self {
        let providers: HashMap<String, bool> =
            provider_names.iter().map(|n| (n.clone(), true)).collect();
        let usage: HashMap<String, TokenUsage> = provider_names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    TokenUsage {
                        provider: n.clone(),
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                )
            })
            .collect();
        Self {
            providers: RwLock::new(providers),
            defaults: RwLock::new(HashMap::new()),
            modalities: provider_modalities,
            usage: RwLock::new(usage),
        }
    }

    pub fn status(&self) -> Vec<ProviderStatus> {
        let providers = self.providers.read().unwrap();
        providers
            .iter()
            .map(|(name, &enabled)| ProviderStatus {
                name: name.clone(),
                enabled,
                modalities: self.modalities.get(name).cloned().unwrap_or_default(),
                is_healthy: enabled, // simplified
            })
            .collect()
    }

    pub fn models(&self, modality_filter: Option<&str>) -> HashMap<String, Vec<String>> {
        let providers = self.providers.read().unwrap();
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for (name, &enabled) in providers.iter() {
            if !enabled {
                continue;
            }
            let mods = self.modalities.get(name).cloned().unwrap_or_default();
            for m in &mods {
                if modality_filter.is_none_or(|f| f == m) {
                    result.entry(m.clone()).or_default().push(name.clone());
                }
            }
        }
        result
    }

    pub fn usage(&self) -> Vec<TokenUsage> {
        self.usage.read().unwrap().values().cloned().collect()
    }

    pub fn disable_provider(&self, name: &str) -> Result<(), String> {
        let mut providers = self.providers.write().unwrap();
        if providers.contains_key(name) {
            providers.insert(name.to_string(), false);
            Ok(())
        } else {
            Err(format!("unknown provider: {name}"))
        }
    }

    pub fn enable_provider(&self, name: &str) -> Result<(), String> {
        let mut providers = self.providers.write().unwrap();
        if providers.contains_key(name) {
            providers.insert(name.to_string(), true);
            Ok(())
        } else {
            Err(format!("unknown provider: {name}"))
        }
    }

    pub fn set_default(&self, modality: &str, provider: &str) -> Result<(), String> {
        // Validate that provider supports this modality
        let supported = self
            .modalities
            .get(provider)
            .map(|m| m.contains(&modality.to_string()))
            .unwrap_or(false);
        if !supported {
            return Err(format!(
                "provider {provider} does not support modality {modality}"
            ));
        }
        self.defaults
            .write()
            .unwrap()
            .insert(modality.to_string(), provider.to_string());
        Ok(())
    }

    pub fn get_default(&self, modality: &str) -> Option<String> {
        self.defaults.read().unwrap().get(modality).cloned()
    }

    pub fn rotate_key(&self, _provider: &str) -> Result<(), String> {
        // Placeholder — in production would update the credential
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler() -> LlmCliHandler {
        let providers = vec!["anthropic".into(), "openai".into()];
        let mut modalities = HashMap::new();
        modalities.insert(
            "anthropic".to_string(),
            vec!["text".to_string(), "vision".to_string()],
        );
        modalities.insert(
            "openai".to_string(),
            vec!["text".to_string(), "embedding".to_string()],
        );
        LlmCliHandler::new(providers, modalities)
    }

    #[test]
    fn test_status_lists_all_providers() {
        let h = make_handler();
        let status = h.status();
        assert_eq!(status.len(), 2);
    }

    #[test]
    fn test_models_groups_by_modality() {
        let h = make_handler();
        let models = h.models(None);
        assert!(models.contains_key("text"));
        assert!(models["text"].contains(&"anthropic".to_string()));
    }

    #[test]
    fn test_models_filtered_by_modality() {
        let h = make_handler();
        let models = h.models(Some("embedding"));
        assert!(models.contains_key("embedding"));
        assert!(!models.contains_key("vision"));
    }

    #[test]
    fn test_usage_non_negative() {
        let h = make_handler();
        for u in h.usage() {
            assert!(u.input_tokens == 0);
            assert!(u.output_tokens == 0);
            assert!(u.total() == 0);
        }
    }

    #[test]
    fn test_disable_provider() {
        let h = make_handler();
        h.disable_provider("anthropic").unwrap();
        let status = h.status();
        let anth = status.iter().find(|s| s.name == "anthropic").unwrap();
        assert!(!anth.enabled);
    }

    #[test]
    fn test_enable_provider() {
        let h = make_handler();
        h.disable_provider("anthropic").unwrap();
        h.enable_provider("anthropic").unwrap();
        let status = h.status();
        let anth = status.iter().find(|s| s.name == "anthropic").unwrap();
        assert!(anth.enabled);
    }

    #[test]
    fn test_set_default_valid() {
        let h = make_handler();
        h.set_default("text", "anthropic").unwrap();
        assert_eq!(h.get_default("text"), Some("anthropic".to_string()));
    }

    #[test]
    fn test_set_default_wrong_modality_rejected() {
        let h = make_handler();
        // anthropic doesn't support "embedding"
        let res = h.set_default("embedding", "anthropic");
        assert!(res.is_err());
    }

    #[test]
    fn test_rotate_completes() {
        let h = make_handler();
        assert!(h.rotate_key("anthropic").is_ok());
    }

    #[test]
    fn test_disabled_provider_not_in_models() {
        let h = make_handler();
        h.disable_provider("openai").unwrap();
        let models = h.models(Some("embedding"));
        // openai supports embedding but is disabled
        assert!(!models
            .get("embedding")
            .is_some_and(|v| v.contains(&"openai".to_string())));
    }

    #[test]
    fn test_disable_unknown_provider() {
        let h = make_handler();
        let res = h.disable_provider("nonexistent");
        assert!(res.is_err());
    }

    #[test]
    fn test_enable_unknown_provider() {
        let h = make_handler();
        let res = h.enable_provider("nonexistent");
        assert!(res.is_err());
    }

    #[test]
    fn test_modalities_shown_in_status() {
        let h = make_handler();
        let status = h.status();
        let anth = status.iter().find(|s| s.name == "anthropic").unwrap();
        assert!(anth.modalities.contains(&"text".to_string()));
        assert!(anth.modalities.contains(&"vision".to_string()));
    }

    #[test]
    fn test_get_default_unset() {
        let h = make_handler();
        assert!(h.get_default("vision").is_none());
    }

    #[test]
    fn test_multiple_defaults() {
        let h = make_handler();
        h.set_default("text", "anthropic").unwrap();
        h.set_default("embedding", "openai").unwrap();
        assert_eq!(h.get_default("text"), Some("anthropic".to_string()));
        assert_eq!(h.get_default("embedding"), Some("openai".to_string()));
    }
}
