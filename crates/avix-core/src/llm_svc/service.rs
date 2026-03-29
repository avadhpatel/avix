use crate::config::{LlmConfig, ProviderAuth};
use crate::error::AvixError;
use crate::ipc::message::{JsonRpcErrorCode, JsonRpcRequest, JsonRpcResponse};
use crate::llm_client::{LlmClient, LlmCompleteRequest};
use crate::llm_svc::adapter::{
    AvixEmbedRequest, AvixImageRequest, AvixSpeechRequest, AvixTranscribeRequest, EmbedInput,
    ProviderAdapter,
};
use crate::llm_svc::http_client::DirectHttpLlmClient;
use crate::llm_svc::routing::RoutingEngine;
use crate::llm_svc::usage::UsageTracker;
use crate::types::Modality;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct CredentialStore {
    pub inner: HashMap<String, ProviderCredential>,
}

#[derive(Clone)]
pub struct ProviderCredential {
    pub header_name: String,
    pub header_value: String,
}

pub struct LlmService {
    config: LlmConfig,
    adapters: HashMap<String, Box<dyn ProviderAdapter>>,
    routing: Arc<RoutingEngine>,
    credentials: Arc<RwLock<CredentialStore>>,
    usage: Arc<UsageTracker>,
    http_client: Arc<reqwest::Client>,
    /// Injected text LLM clients per provider name (provider name → LlmClient)
    text_clients: HashMap<String, Box<dyn LlmClient>>,
}

impl LlmService {
    pub fn new(
        config: LlmConfig,
        adapters: HashMap<String, Box<dyn ProviderAdapter>>,
        routing: Arc<RoutingEngine>,
        text_clients: HashMap<String, Box<dyn LlmClient>>,
    ) -> Self {
        let http_client = Arc::new(reqwest::Client::new());
        let mut text_clients = text_clients;

        // Auto-register a DirectHttpLlmClient for every provider that:
        //   1. supports Modality::Text
        //   2. has a known built-in adapter
        //   3. has credentials available (env var for ApiKey auth, or no auth required)
        //   4. is not already covered by an injected client
        for provider in &config.spec.providers {
            if !provider.modalities.contains(&Modality::Text) {
                continue;
            }
            if text_clients.contains_key(&provider.name) {
                continue;
            }
            let Some(adapter) = Self::make_provider_adapter(&provider.name) else {
                tracing::debug!(
                    provider = %provider.name,
                    "no built-in adapter for text provider — skipping auto-client"
                );
                continue;
            };
            let auth_header = Self::resolve_auth_header(&provider.auth);
            // ApiKey providers require the key to be present; skip if missing.
            if matches!(&provider.auth, ProviderAuth::ApiKey { .. }) && auth_header.is_none() {
                tracing::warn!(
                    provider = %provider.name,
                    "API key env var not set — text client not auto-registered"
                );
                continue;
            }
            let default_model = provider
                .models
                .iter()
                .find(|m| m.modality == Modality::Text)
                .map(|m| m.id.clone())
                .unwrap_or_default();
            let client =
                DirectHttpLlmClient::new(&provider.base_url, &default_model, auth_header, adapter);
            text_clients.insert(provider.name.clone(), Box::new(client));
            tracing::info!(provider = %provider.name, "auto-registered text client from config");
        }

        Self {
            config,
            adapters,
            routing,
            credentials: Arc::new(RwLock::new(CredentialStore {
                inner: HashMap::new(),
            })),
            usage: Arc::new(UsageTracker::new()),
            http_client,
            text_clients,
        }
    }

    /// Returns a fresh `Arc<dyn ProviderAdapter>` for well-known provider names.
    /// Returns `None` for unrecognised names so the caller can skip gracefully.
    fn make_provider_adapter(name: &str) -> Option<Arc<dyn ProviderAdapter>> {
        use crate::llm_svc::adapter::{AnthropicAdapter, OllamaAdapter, OpenAiAdapter, XaiAdapter};
        match name {
            "anthropic" => Some(Arc::new(AnthropicAdapter::new())),
            "openai" => Some(Arc::new(OpenAiAdapter::new())),
            "xai" => Some(Arc::new(XaiAdapter::new())),
            "ollama" => Some(Arc::new(OllamaAdapter::new())),
            _ => None,
        }
    }

    /// Resolves the HTTP auth header for a provider from its `ProviderAuth` config.
    ///
    /// - `ApiKey` — reads the key from the named environment variable; returns `None`
    ///   when the variable is absent so the caller can skip the provider.
    /// - `None` — returns `None` (no header needed; the client is still usable).
    /// - `Oauth2` — not yet handled by `DirectHttpLlmClient`; returns `None`.
    fn resolve_auth_header(auth: &ProviderAuth) -> Option<(String, String)> {
        match auth {
            ProviderAuth::ApiKey {
                secret_name,
                header,
                prefix,
            } => {
                let key = std::env::var(secret_name).ok()?;
                let value = match prefix {
                    Some(p) => format!("{p}{key}"),
                    None => key,
                };
                Some((header.clone(), value))
            }
            ProviderAuth::None | ProviderAuth::Oauth2 { .. } => None,
        }
    }

    pub(crate) async fn dispatch(&self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let result = match req.method.as_str() {
            "llm/complete" => self.handle_complete(&req.params).await,
            "llm/generate-image" => self.handle_generate_image(&req.params).await,
            "llm/generate-speech" => self.handle_generate_speech(&req.params).await,
            "llm/transcribe" => self.handle_transcribe(&req.params).await,
            "llm/embed" => self.handle_embed(&req.params).await,
            "llm/providers" => self.handle_providers().await,
            "llm/usage" => self.handle_usage().await,
            other => Err(AvixError::ConfigParse(format!("unknown method: {other}"))),
        };

        match result {
            Ok(v) => JsonRpcResponse::ok(&req.id, v),
            Err(e) => {
                let code = match &e {
                    AvixError::NoProviderAvailable(_) => JsonRpcErrorCode::Eprovider as i32,
                    AvixError::ProviderNotPermitted(_) => JsonRpcErrorCode::Eprovperm as i32,
                    _ => -32603, // internal error
                };
                JsonRpcResponse::err(&req.id, code, &e.to_string(), None)
            }
        }
    }

    async fn handle_complete(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AvixError> {
        let provider_name = params["provider"].as_str().map(str::to_string);
        let model = params["model"]
            .as_str()
            .unwrap_or("claude-haiku-4-5-20251001")
            .to_string();

        // Resolve provider
        let provider_config = self
            .routing
            .resolve(Modality::Text, provider_name.as_deref())
            .await?;
        let pname = provider_config.name.clone();

        // Build LlmCompleteRequest
        let llm_req = LlmCompleteRequest {
            model: model.clone(),
            messages: params["messages"].as_array().cloned().unwrap_or_default(),
            tools: params["tools"].as_array().cloned().unwrap_or_default(),
            system: params["system"].as_str().map(str::to_string),
            max_tokens: params["maxTokens"].as_u64().unwrap_or(4096) as u32,
        };

        // Use the injected text client for this provider
        let resp = if let Some(client) = self.text_clients.get(&pname) {
            client
                .complete(llm_req)
                .await
                .map_err(|e| AvixError::AdapterError(e.to_string()))?
        } else {
            return Err(AvixError::NoProviderAvailable(format!(
                "no text client for {pname}"
            )));
        };
        tracing::debug!(response =  ?resp, "LLM Complete Response");

        // Record usage
        self.usage
            .record_text(&pname, resp.input_tokens as u64, resp.output_tokens as u64)
            .await;

        Ok(serde_json::json!({
            "provider": pname,
            "model": model,
            "content": resp.content,
            "usage": {
                "inputTokens": resp.input_tokens,
                "outputTokens": resp.output_tokens,
                "totalTokens": resp.total_tokens(),
            },
            "stopReason": format!("{:?}", resp.stop_reason).to_lowercase(),
            "latencyMs": 0u64,
        }))
    }

    async fn handle_generate_image(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AvixError> {
        let provider_name = params["provider"].as_str().map(str::to_string);
        let provider_config = self
            .routing
            .resolve(Modality::Image, provider_name.as_deref())
            .await?;
        let pname = provider_config.name.clone();
        let base_url = provider_config.base_url.clone();

        let adapter = self
            .adapters
            .get(&pname)
            .ok_or_else(|| AvixError::NoProviderAvailable(format!("no adapter for {pname}")))?;

        let req = AvixImageRequest {
            provider: Some(pname.clone()),
            model: params["model"].as_str().unwrap_or("").to_string(),
            prompt: params["prompt"].as_str().unwrap_or("").to_string(),
            negative_prompt: params["negativePrompt"].as_str().map(str::to_string),
            size: params["size"].as_str().map(str::to_string),
            style: params["style"].as_str().map(str::to_string),
            n: params["n"].as_u64().map(|n| n as u32),
            metadata: crate::llm_svc::adapter::CompleteMetadata {
                agent_pid: params["metadata"]["agentPid"].as_u64().unwrap_or(0) as u32,
                session_id: params["metadata"]["sessionId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            },
        };

        let body = adapter
            .build_image_request(&req)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        let agent_pid = req.metadata.agent_pid;
        let model = req.model.clone();

        let creds = self.credentials.read().await;
        let mut request_builder = self
            .http_client
            .post(format!(
                "{}/v1/images/generations",
                base_url.trim_end_matches('/')
            ))
            .json(&body);

        if let Some(cred) = creds.inner.get(&pname) {
            request_builder =
                request_builder.header(cred.header_name.clone(), cred.header_value.clone());
        }
        drop(creds);

        let http_resp = request_builder
            .send()
            .await
            .map_err(|e| AvixError::AdapterError(format!("HTTP error: {e}")))?;
        let raw: serde_json::Value = http_resp
            .json()
            .await
            .map_err(|e| AvixError::AdapterError(format!("JSON parse: {e}")))?;

        let mut outputs = adapter
            .parse_image_response(raw)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        // Write to VFS scratch path
        for output in &mut outputs {
            let ext = if output.mime_type.contains("png") {
                "png"
            } else {
                "jpg"
            };
            output.file_path = format!(
                "/proc/{}/scratch/img-{}.{}",
                agent_pid,
                uuid::Uuid::new_v4(),
                ext
            );
        }

        self.usage.record_image(&pname).await;

        Ok(serde_json::json!({
            "provider": pname,
            "model": model,
            "images": outputs,
            "latencyMs": 0u64,
        }))
    }

    async fn handle_generate_speech(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AvixError> {
        let provider_name = params["provider"].as_str().map(str::to_string);
        let provider_config = self
            .routing
            .resolve(Modality::Speech, provider_name.as_deref())
            .await?;
        let pname = provider_config.name.clone();
        let base_url = provider_config.base_url.clone();

        let adapter = self
            .adapters
            .get(&pname)
            .ok_or_else(|| AvixError::NoProviderAvailable(format!("no adapter for {pname}")))?;

        let req = AvixSpeechRequest {
            provider: Some(pname.clone()),
            model: params["model"].as_str().unwrap_or("").to_string(),
            text: params["text"].as_str().unwrap_or("").to_string(),
            voice: params["voice"].as_str().unwrap_or("").to_string(),
            format: params["format"].as_str().map(str::to_string),
            speed: params["speed"].as_f64().map(|s| s as f32),
            stream: params["stream"].as_bool(),
            metadata: crate::llm_svc::adapter::CompleteMetadata {
                agent_pid: params["metadata"]["agentPid"].as_u64().unwrap_or(0) as u32,
                session_id: params["metadata"]["sessionId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            },
        };

        let endpoint = adapter
            .speech_endpoint(&req)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;
        let body = adapter
            .build_speech_request(&req)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        let url = format!("{}{}", base_url.trim_end_matches('/'), endpoint.url);

        let agent_pid = req.metadata.agent_pid;
        let text_len = req.text.len() as u64;
        let model = req.model.clone();

        let creds = self.credentials.read().await;
        let mut request_builder = self.http_client.post(&url).json(&body);
        if let Some(cred) = creds.inner.get(&pname) {
            request_builder =
                request_builder.header(cred.header_name.clone(), cred.header_value.clone());
        }
        drop(creds);

        let http_resp = request_builder
            .send()
            .await
            .map_err(|e| AvixError::AdapterError(format!("HTTP error: {e}")))?;
        let audio_bytes = http_resp
            .bytes()
            .await
            .map_err(|e| AvixError::AdapterError(format!("bytes error: {e}")))?;

        let vfs_path = format!(
            "/proc/{}/scratch/speech-{}.{}",
            agent_pid,
            uuid::Uuid::new_v4(),
            endpoint.format
        );

        self.usage.record_speech(&pname, text_len).await;

        Ok(serde_json::json!({
            "provider": pname,
            "model": model,
            "filePath": vfs_path,
            "mimeType": format!("audio/{}", endpoint.format),
            "bytes": audio_bytes.len() as u64,
            "latencyMs": 0u64,
        }))
    }

    async fn handle_transcribe(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AvixError> {
        let provider_name = params["provider"].as_str().map(str::to_string);
        let provider_config = self
            .routing
            .resolve(Modality::Transcription, provider_name.as_deref())
            .await?;
        let pname = provider_config.name.clone();
        let base_url = provider_config.base_url.clone();

        let adapter = self
            .adapters
            .get(&pname)
            .ok_or_else(|| AvixError::NoProviderAvailable(format!("no adapter for {pname}")))?;

        let req = AvixTranscribeRequest {
            provider: Some(pname.clone()),
            model: params["model"].as_str().unwrap_or("whisper-1").to_string(),
            file_path: params["filePath"].as_str().unwrap_or("").to_string(),
            language: params["language"].as_str().map(str::to_string),
            prompt: params["prompt"].as_str().map(str::to_string),
            granularity: params["granularity"].as_str().map(str::to_string),
            metadata: crate::llm_svc::adapter::CompleteMetadata {
                agent_pid: params["metadata"]["agentPid"].as_u64().unwrap_or(0) as u32,
                session_id: params["metadata"]["sessionId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            },
        };

        let model = req.model.clone();

        // Build multipart request (empty audio bytes — VFS read would go here)
        let audio_bytes: Vec<u8> = vec![];
        let mp = adapter
            .build_transcription_request(&req, &audio_bytes)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        let url = format!("{}{}", base_url.trim_end_matches('/'), mp.url);

        let creds = self.credentials.read().await;
        let mut form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(mp.audio_bytes.clone()),
            )
            .text("model", model);
        for (k, v) in &mp.fields {
            form = form.text(k.clone(), v.clone());
        }
        let mut request_builder = self.http_client.post(&url).multipart(form);
        if let Some(cred) = creds.inner.get(&pname) {
            request_builder =
                request_builder.header(cred.header_name.clone(), cred.header_value.clone());
        }
        drop(creds);

        let http_resp = request_builder
            .send()
            .await
            .map_err(|e| AvixError::AdapterError(format!("HTTP error: {e}")))?;
        let raw: serde_json::Value = http_resp
            .json()
            .await
            .map_err(|e| AvixError::AdapterError(format!("JSON parse: {e}")))?;

        let result = adapter
            .parse_transcription_response(raw)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        self.usage
            .record_transcription(&pname, result.duration_sec.unwrap_or(0.0) as f64)
            .await;

        serde_json::to_value(&result).map_err(|e| AvixError::AdapterError(e.to_string()))
    }

    async fn handle_embed(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, AvixError> {
        let provider_name = params["provider"].as_str().map(str::to_string);
        let provider_config = self
            .routing
            .resolve(Modality::Embedding, provider_name.as_deref())
            .await?;
        let pname = provider_config.name.clone();
        let base_url = provider_config.base_url.clone();

        let adapter = self
            .adapters
            .get(&pname)
            .ok_or_else(|| AvixError::NoProviderAvailable(format!("no adapter for {pname}")))?;

        let input = if let Some(arr) = params["input"].as_array() {
            EmbedInput::Batch(
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            )
        } else if let Some(s) = params["input"].as_str() {
            EmbedInput::Single(s.to_string())
        } else {
            return Err(AvixError::AdapterError("missing input".into()));
        };

        let req = AvixEmbedRequest {
            provider: Some(pname.clone()),
            model: params["model"]
                .as_str()
                .unwrap_or("text-embedding-3-small")
                .to_string(),
            input,
            metadata: crate::llm_svc::adapter::CompleteMetadata {
                agent_pid: params["metadata"]["agentPid"].as_u64().unwrap_or(0) as u32,
                session_id: params["metadata"]["sessionId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            },
        };

        let body = adapter
            .build_embed_request(&req)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        let url = format!("{}/v1/embeddings", base_url.trim_end_matches('/'));

        let creds = self.credentials.read().await;
        let mut request_builder = self.http_client.post(&url).json(&body);
        if let Some(cred) = creds.inner.get(&pname) {
            request_builder =
                request_builder.header(cred.header_name.clone(), cred.header_value.clone());
        }
        drop(creds);

        let http_resp = request_builder
            .send()
            .await
            .map_err(|e| AvixError::AdapterError(format!("HTTP error: {e}")))?;
        let raw: serde_json::Value = http_resp
            .json()
            .await
            .map_err(|e| AvixError::AdapterError(format!("JSON parse: {e}")))?;

        let result = adapter
            .parse_embed_response(raw)
            .map_err(|e| AvixError::AdapterError(e.to_string()))?;

        self.usage
            .record_embedding(&pname, result.usage.input_tokens as u64)
            .await;

        serde_json::to_value(&result).map_err(|e| AvixError::AdapterError(e.to_string()))
    }

    async fn handle_providers(&self) -> Result<serde_json::Value, AvixError> {
        let statuses = self.routing.all_statuses().await;
        let providers: Vec<serde_json::Value> = self
            .config
            .spec
            .providers
            .iter()
            .map(|p| {
                let status = statuses.get(&p.name);
                let (status_str, last_error) = match status {
                    Some(crate::llm_svc::routing::ProviderStatus::Available) | None => {
                        ("available", None)
                    }
                    Some(crate::llm_svc::routing::ProviderStatus::Degraded { reason }) => {
                        ("degraded", Some(reason.clone()))
                    }
                    Some(crate::llm_svc::routing::ProviderStatus::Unavailable { reason }) => {
                        ("unavailable", Some(reason.clone()))
                    }
                };
                serde_json::json!({
                    "name": p.name,
                    "status": status_str,
                    "modalities": p.modalities.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
                    "models": p.models.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                    "authType": match &p.auth {
                        crate::config::ProviderAuth::ApiKey { .. } => "api_key",
                        crate::config::ProviderAuth::Oauth2 { .. } => "oauth2",
                        crate::config::ProviderAuth::None => "none",
                    },
                    "lastError": last_error,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "providers": providers,
            "defaultProviders": {
                "text": self.config.spec.default_providers.text,
                "image": self.config.spec.default_providers.image,
                "speech": self.config.spec.default_providers.speech,
                "transcription": self.config.spec.default_providers.transcription,
                "embedding": self.config.spec.default_providers.embedding,
            }
        }))
    }

    async fn handle_usage(&self) -> Result<serde_json::Value, AvixError> {
        let snapshot = self.usage.snapshot().await;
        serde_json::to_value(&snapshot).map_err(|e| AvixError::AdapterError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LlmConfig;
    use crate::llm_svc::routing::RoutingEngine;

    fn make_two_provider_config() -> LlmConfig {
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

    fn make_service() -> LlmService {
        let config = make_two_provider_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        LlmService::new(config, HashMap::new(), routing, HashMap::new())
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_unknown_method() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-1".into(),
            method: "unknown/method".into(),
            params: serde_json::json!({}),
        };
        let resp = svc.dispatch(&req).await;
        assert!(resp.error.is_some(), "expected error for unknown method");
        let err = resp.error.unwrap();
        assert!(
            err.message.contains("unknown method"),
            "msg: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn test_llm_service_handle_providers_empty() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-2".into(),
            method: "llm/providers".into(),
            params: serde_json::json!({}),
        };
        let resp = svc.dispatch(&req).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let providers = result["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 2);
        // Check that default providers are returned
        assert_eq!(result["defaultProviders"]["text"], "anthropic");
        assert_eq!(result["defaultProviders"]["image"], "openai");
    }

    #[tokio::test]
    async fn test_llm_service_handle_usage_empty() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-3".into(),
            method: "llm/usage".into(),
            params: serde_json::json!({}),
        };
        let resp = svc.dispatch(&req).await;
        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        // Empty usage returns an empty object
        let result = resp.result.unwrap();
        assert!(result.is_object());
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_llm_complete_no_client_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-4".into(),
            method: "llm/complete".into(),
            params: serde_json::json!({
                "provider": "anthropic",
                "model": "claude-3",
                "messages": [],
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
            }),
        };
        let resp = svc.dispatch(&req).await;
        // No text client registered for anthropic → should return an error
        assert!(
            resp.error.is_some(),
            "expected error when no text client is registered"
        );
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_llm_complete_unknown_provider_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-5".into(),
            method: "llm/complete".into(),
            params: serde_json::json!({
                "provider": "nonexistent-provider",
                "model": "model-x",
                "messages": [],
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
            }),
        };
        let resp = svc.dispatch(&req).await;
        assert!(resp.error.is_some(), "expected error for unknown provider");
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_generate_image_no_adapter_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-6".into(),
            method: "llm/generate-image".into(),
            params: serde_json::json!({
                "provider": "openai",
                "model": "dall-e-3",
                "prompt": "a cat",
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
            }),
        };
        let resp = svc.dispatch(&req).await;
        // No adapter registered → error
        assert!(
            resp.error.is_some(),
            "expected error when no image adapter is registered"
        );
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_generate_speech_no_adapter_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-7".into(),
            method: "llm/generate-speech".into(),
            params: serde_json::json!({
                "provider": "openai",
                "model": "tts-1",
                "text": "hello",
                "voice": "alloy",
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
            }),
        };
        let resp = svc.dispatch(&req).await;
        assert!(
            resp.error.is_some(),
            "expected error when no speech adapter is registered"
        );
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_transcribe_no_adapter_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-8".into(),
            method: "llm/transcribe".into(),
            params: serde_json::json!({
                "provider": "openai",
                "model": "whisper-1",
                "filePath": "/tmp/audio.wav",
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
            }),
        };
        let resp = svc.dispatch(&req).await;
        assert!(
            resp.error.is_some(),
            "expected error when no transcription adapter is registered"
        );
    }

    #[tokio::test]
    async fn test_llm_service_dispatch_embed_no_input_returns_error() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-9".into(),
            method: "llm/embed".into(),
            params: serde_json::json!({
                "provider": "openai",
                "model": "text-embedding-3-small",
                "metadata": {
                    "agentPid": 1,
                    "sessionId": "sess-test"
                }
                // no "input" field
            }),
        };
        let resp = svc.dispatch(&req).await;
        assert!(
            resp.error.is_some(),
            "expected error when missing embed input"
        );
    }

    #[tokio::test]
    async fn test_credential_store_holds_values() {
        let store = CredentialStore {
            inner: {
                let mut m = HashMap::new();
                m.insert(
                    "anthropic".to_string(),
                    ProviderCredential {
                        header_name: "x-api-key".to_string(),
                        header_value: "sk-test-123".to_string(),
                    },
                );
                m
            },
        };
        let cred = store.inner.get("anthropic").unwrap();
        assert_eq!(cred.header_name, "x-api-key");
        assert_eq!(cred.header_value, "sk-test-123");
    }

    #[tokio::test]
    async fn test_provider_credential_clone() {
        let cred = ProviderCredential {
            header_name: "Authorization".to_string(),
            header_value: "Bearer tok".to_string(),
        };
        let cloned = cred.clone();
        assert_eq!(cloned.header_name, "Authorization");
        assert_eq!(cloned.header_value, "Bearer tok");
    }

    #[tokio::test]
    async fn test_llm_service_providers_includes_modalities_and_auth() {
        let svc = make_service();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-10".into(),
            method: "llm/providers".into(),
            params: serde_json::json!({}),
        };
        let resp = svc.dispatch(&req).await;
        let result = resp.result.unwrap();
        let providers = result["providers"].as_array().unwrap();
        // anthropic provider should have authType: api_key
        let anthropic = providers.iter().find(|p| p["name"] == "anthropic").unwrap();
        assert_eq!(anthropic["authType"], "api_key");
        assert_eq!(anthropic["status"], "available");
    }

    #[tokio::test]
    async fn test_llm_service_new_creates_instance() {
        let config = make_two_provider_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let svc = LlmService::new(config, HashMap::new(), routing, HashMap::new());
        // verify the service was created by dispatching a basic request
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "req-init".into(),
            method: "llm/usage".into(),
            params: serde_json::json!({}),
        };
        let resp = svc.dispatch(&req).await;
        assert!(resp.error.is_none());
    }

    // ── auto-population tests ─────────────────────────────────────────────────

    fn make_ollama_config() -> LlmConfig {
        LlmConfig::from_str(
            r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: ollama
    image: ollama
    speech: ollama
    transcription: ollama
    embedding: ollama
  providers:
    - name: ollama
      baseUrl: http://localhost:11434
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: none
"#,
        )
        .unwrap()
    }

    #[test]
    fn test_new_auto_registers_text_client_for_no_auth_provider() {
        // Ollama uses ProviderAuth::None — no env var required.
        // new() must auto-build and insert a text client for it.
        let config = make_ollama_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let svc = LlmService::new(config, HashMap::new(), routing, HashMap::new());
        assert!(
            svc.text_clients.contains_key("ollama"),
            "text_clients should contain 'ollama' after auto-registration"
        );
    }

    #[test]
    fn test_new_skips_api_key_provider_when_env_var_absent() {
        // anthropic uses ApiKey auth; ANTHROPIC_API_KEY is not set in the test
        // environment, so no client should be auto-registered.
        let config = make_two_provider_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        // Guard: if someone runs tests with the key actually set the assertion is
        // misleading — skip rather than fail.
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            return;
        }
        let svc = LlmService::new(config, HashMap::new(), routing, HashMap::new());
        assert!(
            !svc.text_clients.contains_key("anthropic"),
            "anthropic client must not be registered when ANTHROPIC_API_KEY is absent"
        );
    }

    #[test]
    fn test_new_does_not_overwrite_injected_client() {
        use crate::llm_client::{LlmCompleteRequest, LlmCompleteResponse, StopReason};
        use async_trait::async_trait;

        struct SentinelClient;
        #[async_trait]
        impl LlmClient for SentinelClient {
            async fn complete(&self, _: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
                Ok(LlmCompleteResponse {
                    content: vec![],
                    stop_reason: StopReason::EndTurn,
                    input_tokens: 0,
                    output_tokens: 0,
                })
            }
        }

        // Inject a sentinel client for "ollama" before calling new().
        let config = make_ollama_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let mut pre_injected: HashMap<String, Box<dyn LlmClient>> = HashMap::new();
        pre_injected.insert("ollama".to_string(), Box::new(SentinelClient));

        let svc = LlmService::new(config, HashMap::new(), routing, pre_injected);
        // The injected client must still be present; auto-population must not replace it.
        assert!(svc.text_clients.contains_key("ollama"));
    }

    #[test]
    fn test_new_skips_unknown_provider_name() {
        // A provider named "custom-llm" has no built-in adapter; it must be silently
        // skipped without panicking.
        let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: custom-llm
    image: custom-llm
    speech: custom-llm
    transcription: custom-llm
    embedding: custom-llm
  providers:
    - name: custom-llm
      baseUrl: http://localhost:9999
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: none
"#;
        let config = LlmConfig::from_str(yaml).unwrap();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let svc = LlmService::new(config, HashMap::new(), routing, HashMap::new());
        assert!(
            !svc.text_clients.contains_key("custom-llm"),
            "unknown provider should not have a text client registered"
        );
    }

    #[test]
    fn test_resolve_auth_header_api_key_with_prefix() {
        // Set a temporary env var for the test.
        std::env::set_var("TEST_RESOLVE_AUTH_KEY", "mykey123");
        let auth = ProviderAuth::ApiKey {
            secret_name: "TEST_RESOLVE_AUTH_KEY".to_string(),
            header: "Authorization".to_string(),
            prefix: Some("Bearer ".to_string()),
        };
        let result = LlmService::resolve_auth_header(&auth);
        std::env::remove_var("TEST_RESOLVE_AUTH_KEY");
        let (name, value) = result.expect("should resolve header");
        assert_eq!(name, "Authorization");
        assert_eq!(value, "Bearer mykey123");
    }

    #[test]
    fn test_resolve_auth_header_api_key_without_prefix() {
        std::env::set_var("TEST_RESOLVE_AUTH_KEY_BARE", "rawkey");
        let auth = ProviderAuth::ApiKey {
            secret_name: "TEST_RESOLVE_AUTH_KEY_BARE".to_string(),
            header: "x-api-key".to_string(),
            prefix: None,
        };
        let result = LlmService::resolve_auth_header(&auth);
        std::env::remove_var("TEST_RESOLVE_AUTH_KEY_BARE");
        let (name, value) = result.expect("should resolve header");
        assert_eq!(name, "x-api-key");
        assert_eq!(value, "rawkey");
    }

    #[test]
    fn test_resolve_auth_header_missing_env_var_returns_none() {
        std::env::remove_var("NONEXISTENT_KEY_XYZ");
        let auth = ProviderAuth::ApiKey {
            secret_name: "NONEXISTENT_KEY_XYZ".to_string(),
            header: "Authorization".to_string(),
            prefix: None,
        };
        assert!(LlmService::resolve_auth_header(&auth).is_none());
    }

    #[test]
    fn test_resolve_auth_header_none_auth_returns_none() {
        assert!(LlmService::resolve_auth_header(&ProviderAuth::None).is_none());
    }

    #[test]
    fn test_make_provider_adapter_known_providers() {
        for name in &["anthropic", "openai", "xai", "ollama"] {
            assert!(
                LlmService::make_provider_adapter(name).is_some(),
                "expected adapter for '{name}'"
            );
        }
    }

    #[test]
    fn test_make_provider_adapter_unknown_returns_none() {
        assert!(LlmService::make_provider_adapter("unknown-provider").is_none());
    }
}
