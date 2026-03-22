use std::sync::Arc;

use async_trait::async_trait;
use autoagents::llm::chat::{ChatMessage, ChatProvider, ChatRole, FunctionTool, MessageType, Tool};
use serde_json::{json, Value};

use crate::llm_client::{LlmCompleteRequest, LlmCompleteResponse, StopReason};

/// Adapts any AutoAgents `ChatProvider` into Avix's `LlmClient` trait.
///
/// `build()` on AutoAgents' `LLMBuilder` returns `Arc<T>`, so we store `Arc<P>`
/// internally and reach `P::ChatProvider` methods via Deref coercion rather than
/// requiring a `Arc<T>: ChatProvider` blanket impl (which the library doesn't provide).
pub struct AutoAgentsChatClient<P: ChatProvider> {
    provider: Arc<P>,
}

impl<P: ChatProvider> AutoAgentsChatClient<P> {
    /// `provider` is the `Arc<P>` returned by `LLMBuilder::<P>::new()...build()`.
    pub fn new(provider: Arc<P>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl<P: ChatProvider + Send + Sync + 'static> crate::llm_client::LlmClient
    for AutoAgentsChatClient<P>
{
    async fn complete(&self, req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        let mut messages: Vec<ChatMessage> = Vec::new();

        // System prompt prepended as a ChatRole::System message
        if let Some(sys) = &req.system {
            messages.push(ChatMessage {
                role: ChatRole::System,
                message_type: MessageType::Text,
                content: sys.clone(),
            });
        }

        for msg in &req.messages {
            let role = match msg["role"].as_str() {
                Some("user") => ChatRole::User,
                Some("assistant") => ChatRole::Assistant,
                Some("system") => ChatRole::System,
                _ => ChatRole::User,
            };
            let content = match &msg["content"] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            messages.push(ChatMessage {
                role,
                message_type: MessageType::Text,
                content,
            });
        }

        // Convert Avix tool descriptors (Anthropic wire format) → AutoAgents Tool
        let tools: Vec<Tool> = req
            .tools
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_string();
                let description = t["description"].as_str().unwrap_or("").to_string();
                let parameters = t.get("input_schema").cloned().unwrap_or(json!({}));
                Some(Tool {
                    tool_type: "function".to_string(),
                    function: FunctionTool {
                        name,
                        description,
                        parameters,
                    },
                })
            })
            .collect();

        let tool_slice = if tools.is_empty() {
            None
        } else {
            Some(tools.as_slice())
        };

        // Deref Arc<P> → &P, then call P's ChatProvider impl
        let response = self
            .provider
            .chat_with_tools(&messages, tool_slice, None)
            .await
            .map_err(|e| anyhow::anyhow!("LLM error: {e}"))?;

        let text_content = response.text();
        let tool_calls = response.tool_calls();
        let usage = response.usage();

        let mut content: Vec<Value> = Vec::new();
        if let Some(text) = text_content {
            content.push(json!({ "type": "text", "text": text }));
        }

        let stop_reason = if let Some(calls) = tool_calls {
            for call in &calls {
                // Unmangle wire format back to Avix format (fs__read → fs/read)
                let name = call.function.name.replace("__", "/");
                let args: Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or(json!({}));
                content.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": name,
                    "input": args,
                }));
            }
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        let (input_tokens, output_tokens) = usage
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .unwrap_or((0, 0));

        Ok(LlmCompleteResponse {
            content,
            stop_reason,
            input_tokens,
            output_tokens,
        })
    }
}
