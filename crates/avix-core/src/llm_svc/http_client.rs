use crate::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse, StopReason};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct AnthropicHttpClient {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicHttpClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_model(api_key, "claude-sonnet-4-5")
    }

    pub fn with_model(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicHttpClient {
    async fn complete(&self, req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        let model = if req.model.is_empty() {
            self.model.clone()
        } else {
            req.model.clone()
        };

        let mut body = json!({
            "model": model,
            "max_tokens": req.max_tokens,
            "messages": req.messages,
        });

        if let Some(system) = &req.system {
            body["system"] = json!(system);
        }
        if !req.tools.is_empty() {
            body["tools"] = json!(req.tools);
        }

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body: Value = resp.json().await?;

        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            anyhow::bail!("Anthropic API error {status}: {msg}");
        }

        let stop_reason: StopReason = body["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .parse()
            .unwrap_or(StopReason::EndTurn);

        let content = body["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let input_tokens = body["usage"]["input_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens = body["usage"]["output_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;

        Ok(LlmCompleteResponse {
            content,
            stop_reason,
            input_tokens,
            output_tokens,
        })
    }
}
