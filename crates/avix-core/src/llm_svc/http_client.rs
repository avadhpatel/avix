use crate::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse, StopReason, StreamChunk};
use crate::llm_svc::adapter::{
    AvixCompleteRequest, AvixToolDescriptor, CompleteMetadata, ProviderAdapter,
};
use crate::llm_svc::sse::{sse_lines, SseLine};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, warn};

/// A `LlmClient` that talks directly to any OpenAI-compatible HTTP endpoint.
///
/// This bypasses the autoagents library entirely and is required for providers
/// (such as xAI/Grok) whose autoagents backend does not implement tool calling.
/// It uses the provider's `ProviderAdapter` for all request/response translation
/// so tool-name mangling, message formatting, etc. are handled correctly.
pub struct DirectHttpLlmClient {
    base_url: String,
    /// Default model used when `LlmCompleteRequest.model` is empty.
    /// The `RuntimeExecutor` sends an empty model string and expects the client
    /// to know its own default.
    default_model: String,
    /// `(header_name, header_value)` — None for unauthenticated providers (e.g. Ollama).
    auth_header: Option<(String, String)>,
    adapter: Arc<dyn ProviderAdapter>,
    http_client: reqwest::Client,
}

impl DirectHttpLlmClient {
    pub fn new(
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        auth_header: Option<(String, String)>,
        adapter: Arc<dyn ProviderAdapter>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            default_model: default_model.into(),
            auth_header,
            adapter,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for DirectHttpLlmClient {
    async fn complete(&self, req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        // Convert the raw tool JSON (Avix internal format) into typed AvixToolDescriptors.
        let tools: Vec<AvixToolDescriptor> = req
            .tools
            .iter()
            .filter_map(|t| {
                Some(AvixToolDescriptor {
                    name: t["name"].as_str()?.to_string(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: t
                        .get("input_schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                })
            })
            .collect();

        // For OpenAI-compatible APIs the system prompt is a role:system message,
        // not a separate field. Prepend it to the messages array if present.
        let mut messages = req.messages;
        if let Some(sys) = &req.system {
            messages.insert(0, json!({"role": "system", "content": sys}));
        }

        // RuntimeExecutor sends an empty model string — fall back to the client's default.
        let model = if req.model.is_empty() {
            self.default_model.clone()
        } else {
            req.model
        };

        let avix_req = AvixCompleteRequest {
            provider: None,
            model,
            messages,
            system: None, // already injected above
            max_tokens: Some(req.max_tokens),
            temperature: None,
            stream: Some(false),
            stop_sequences: None,
            tools,
            metadata: CompleteMetadata {
                agent_pid: 0,
                session_id: String::new(),
            },
        };

        let body = self.adapter.build_complete_request(&avix_req);

        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.adapter.complete_path()
        );

        debug!(
            provider = self.adapter.provider_name(),
            url = %url,
            body = %body,
            "DirectHttpLlmClient → request"
        );

        let mut builder = self.http_client.post(&url).json(&body);
        if let Some((name, value)) = &self.auth_header {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let http_resp = builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP error calling {url}: {e}"))?;

        let status = http_resp.status();
        let raw: serde_json::Value = http_resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("failed to parse JSON response from {url}: {e}"))?;

        debug!(
            provider = self.adapter.provider_name(),
            status = status.as_u16(),
            body = %raw,
            "DirectHttpLlmClient ← response"
        );

        if !status.is_success() {
            // Extract the error message; fall back to the full raw body so there
            // is always something actionable to debug.
            let msg = raw["error"]["message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| raw.to_string());
            warn!(
                provider = self.adapter.provider_name(),
                status = status.as_u16(),
                error = %msg,
                "provider request failed"
            );
            return Err(anyhow::anyhow!(
                "provider {} returned {}: {}",
                self.adapter.provider_name(),
                status,
                msg
            ));
        }

        let avix_resp = self
            .adapter
            .parse_complete_response(raw)
            .map_err(|e| anyhow::anyhow!("failed to parse provider response: {e}"))?;

        let stop_reason = match avix_resp.stop_reason.as_str() {
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        };

        Ok(LlmCompleteResponse {
            content: avix_resp.content,
            stop_reason,
            input_tokens: avix_resp.usage.input_tokens,
            output_tokens: avix_resp.usage.output_tokens,
        })
    }

    async fn stream_complete(
        &self,
        req: LlmCompleteRequest,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamChunk>>> {
        // Convert raw tool JSON into typed descriptors.
        let tools: Vec<AvixToolDescriptor> = req
            .tools
            .iter()
            .filter_map(|t| {
                Some(AvixToolDescriptor {
                    name: t["name"].as_str()?.to_string(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: t
                        .get("input_schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                })
            })
            .collect();

        let mut messages = req.messages;
        if let Some(sys) = &req.system {
            messages.insert(0, json!({"role": "system", "content": sys}));
        }

        let model = if req.model.is_empty() {
            self.default_model.clone()
        } else {
            req.model
        };

        let avix_req = AvixCompleteRequest {
            provider: None,
            model,
            messages,
            system: None,
            max_tokens: Some(req.max_tokens),
            temperature: None,
            stream: Some(true),
            stop_sequences: None,
            tools,
            metadata: CompleteMetadata {
                agent_pid: 0,
                session_id: String::new(),
            },
        };

        let body = self.adapter.build_stream_request(&avix_req);

        let url = format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            self.adapter.stream_complete_path()
        );

        debug!(
            provider = self.adapter.provider_name(),
            url = %url,
            "DirectHttpLlmClient → stream request"
        );

        let mut builder = self.http_client.post(&url).json(&body);
        if let Some((name, value)) = &self.auth_header {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let http_resp = builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP error calling {url}: {e}"))?;

        let status = http_resp.status();
        if !status.is_success() {
            let text = http_resp.text().await.unwrap_or_default();
            warn!(
                provider = self.adapter.provider_name(),
                status = status.as_u16(),
                error = %text,
                "streaming provider request failed"
            );
            return Err(anyhow::anyhow!(
                "provider {} returned {}: {}",
                self.adapter.provider_name(),
                status,
                text
            ));
        }

        // Decode the SSE byte stream into StreamChunks.
        let adapter = Arc::clone(&self.adapter);
        let byte_stream = http_resp.bytes_stream();
        let line_stream = sse_lines(byte_stream);

        let chunk_stream = line_stream
            .scan(None::<String>, move |last_event, line_result| {
                let adapter = Arc::clone(&adapter);
                let line = match line_result {
                    Ok(l) => l,
                    Err(e) => {
                        return futures::future::ready(Some(Some(Err(e))));
                    }
                };

                match line {
                    SseLine::Done => futures::future::ready(Some(None)),
                    SseLine::Event(name) => {
                        *last_event = Some(name);
                        futures::future::ready(Some(None))
                    }
                    SseLine::Data(data) => {
                        let event_name = last_event.as_deref();
                        let result = adapter.parse_stream_event(event_name, &data)
                            .map_err(|e| anyhow::anyhow!("SSE parse error: {e}"));
                        *last_event = None;
                        match result {
                            Ok(Some(chunk)) => futures::future::ready(Some(Some(Ok(chunk)))),
                            Ok(None) => futures::future::ready(Some(None)),
                            Err(e) => futures::future::ready(Some(Some(Err(e)))),
                        }
                    }
                }
            })
            .filter_map(futures::future::ready);

        Ok(Box::pin(chunk_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_svc::adapter::xai::XaiAdapter;
    use crate::types::Modality;
    use std::sync::Arc;

    fn make_client() -> DirectHttpLlmClient {
        DirectHttpLlmClient::new(
            "https://api.x.ai",
            "grok-3-mini",
            Some(("Authorization".to_string(), "Bearer test-key".to_string())),
            Arc::new(XaiAdapter::new()),
        )
    }

    #[test]
    fn test_direct_http_client_fields() {
        let client = make_client();
        assert_eq!(client.base_url, "https://api.x.ai");
        assert_eq!(client.default_model, "grok-3-mini");
        assert!(client.auth_header.is_some());
        let (name, value) = client.auth_header.as_ref().unwrap();
        assert_eq!(name, "Authorization");
        assert!(value.starts_with("Bearer "));
    }

    #[test]
    fn test_direct_http_client_no_auth() {
        let client = DirectHttpLlmClient::new(
            "http://localhost:11434",
            "llama3.2",
            None,
            Arc::new(crate::llm_svc::adapter::OllamaAdapter::new()),
        );
        assert!(client.auth_header.is_none());
        assert_eq!(client.default_model, "llama3.2");
    }

    #[test]
    fn test_adapter_provider_name() {
        let client = make_client();
        assert_eq!(client.adapter.provider_name(), "xai");
    }

    #[test]
    fn test_adapter_modalities() {
        let client = make_client();
        assert!(client.adapter.modalities().contains(&Modality::Text));
    }
}
