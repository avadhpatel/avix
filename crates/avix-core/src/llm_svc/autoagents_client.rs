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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::LlmClient;
    use async_trait::async_trait;
    use autoagents::llm::chat::{
        ChatMessage, ChatProvider, ChatResponse, StructuredOutputFormat, Tool, Usage,
    };
    use autoagents::llm::{FunctionCall, ToolCall};
    use autoagents::llm::error::LLMError;

    #[derive(Debug)]
    struct MockChatResponse {
        text: Option<String>,
        tool_calls: Option<Vec<ToolCall>>,
        usage: Option<Usage>,
    }

    impl std::fmt::Display for MockChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "MockChatResponse")
        }
    }

    impl ChatResponse for MockChatResponse {
        fn text(&self) -> Option<String> {
            self.text.clone()
        }
        fn tool_calls(&self) -> Option<Vec<ToolCall>> {
            self.tool_calls.clone()
        }
        fn usage(&self) -> Option<Usage> {
            self.usage.clone()
        }
    }

    struct MockProvider {
        response_text: String,
    }

    #[async_trait]
    impl ChatProvider for MockProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[Tool]>,
            _json_schema: Option<StructuredOutputFormat>,
        ) -> Result<Box<dyn ChatResponse>, LLMError> {
            Ok(Box::new(MockChatResponse {
                text: Some(self.response_text.clone()),
                tool_calls: None,
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    completion_tokens_details: None,
                    prompt_tokens_details: None,
                }),
            }))
        }
    }

    struct MockProviderWithToolCall;

    #[async_trait]
    impl ChatProvider for MockProviderWithToolCall {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[Tool]>,
            _json_schema: Option<StructuredOutputFormat>,
        ) -> Result<Box<dyn ChatResponse>, LLMError> {
            Ok(Box::new(MockChatResponse {
                text: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call-1".to_string(),
                    call_type: "function".to_string(),
                    function: FunctionCall {
                        name: "fs__read".to_string(),
                        arguments: r#"{"path":"/tmp/test"}"#.to_string(),
                    },
                }]),
                usage: None,
            }))
        }
    }

    #[tokio::test]
    async fn test_autoagents_client_text_response() {
        let provider = Arc::new(MockProvider {
            response_text: "Hello from mock".to_string(),
        });
        let client = AutoAgentsChatClient::new(provider);

        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
            tools: vec![],
            system: None,
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(resp.input_tokens, 10);
        assert_eq!(resp.output_tokens, 5);
        // Content should have the text block
        let text_block = resp.content.iter().find(|c| c["type"] == "text").unwrap();
        assert_eq!(text_block["text"], "Hello from mock");
    }

    #[tokio::test]
    async fn test_autoagents_client_with_system_prompt() {
        let provider = Arc::new(MockProvider {
            response_text: "System response".to_string(),
        });
        let client = AutoAgentsChatClient::new(provider);

        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            tools: vec![],
            system: Some("You are a helpful assistant.".to_string()),
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn test_autoagents_client_tool_call_response() {
        let provider = Arc::new(MockProviderWithToolCall);
        let client = AutoAgentsChatClient::new(provider);

        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "read a file"})],
            tools: vec![serde_json::json!({
                "name": "fs/read",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            })],
            system: None,
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        // Should have tool use stop reason
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        // Should have a tool_use block with unmangled name
        let tool_block = resp
            .content
            .iter()
            .find(|c| c["type"] == "tool_use")
            .unwrap();
        assert_eq!(tool_block["name"], "fs/read"); // unmangled from fs__read
        assert_eq!(tool_block["id"], "call-1");
    }

    #[tokio::test]
    async fn test_autoagents_client_multiple_message_roles() {
        let provider = Arc::new(MockProvider {
            response_text: "ok".to_string(),
        });
        let client = AutoAgentsChatClient::new(provider);

        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![
                serde_json::json!({"role": "user", "content": "hi"}),
                serde_json::json!({"role": "assistant", "content": "hello"}),
                serde_json::json!({"role": "system", "content": "system msg"}),
                serde_json::json!({"role": "unknown_role", "content": "??"}),
            ],
            tools: vec![],
            system: None,
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn test_autoagents_client_no_usage_returns_zero_tokens() {
        struct NoUsageProvider;

        #[derive(Debug)]
        struct NoUsageResponse;
        impl std::fmt::Display for NoUsageResponse {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "NoUsageResponse")
            }
        }
        impl ChatResponse for NoUsageResponse {
            fn text(&self) -> Option<String> {
                Some("hi".to_string())
            }
            fn tool_calls(&self) -> Option<Vec<ToolCall>> {
                None
            }
        }

        #[async_trait]
        impl ChatProvider for NoUsageProvider {
            async fn chat_with_tools(
                &self,
                _m: &[ChatMessage],
                _t: Option<&[Tool]>,
                _js: Option<StructuredOutputFormat>,
            ) -> Result<Box<dyn ChatResponse>, LLMError> {
                Ok(Box::new(NoUsageResponse))
            }
        }

        let client = AutoAgentsChatClient::new(Arc::new(NoUsageProvider));
        let req = LlmCompleteRequest {
            model: "m".to_string(),
            messages: vec![],
            tools: vec![],
            system: None,
            max_tokens: 16,
        };
        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.input_tokens, 0);
        assert_eq!(resp.output_tokens, 0);
    }

    #[tokio::test]
    async fn test_autoagents_client_with_tools_passed() {
        let provider = Arc::new(MockProvider {
            response_text: "done".to_string(),
        });
        let client = AutoAgentsChatClient::new(provider);

        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "do it"})],
            tools: vec![
                serde_json::json!({
                    "name": "fs/read",
                    "description": "Read file",
                    "input_schema": {}
                }),
                serde_json::json!({
                    "name": "fs/write",
                    "description": "Write file"
                    // no input_schema key — tests the unwrap_or path
                }),
            ],
            system: None,
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[tokio::test]
    async fn test_autoagents_client_json_content_value() {
        let provider = Arc::new(MockProvider {
            response_text: "ok".to_string(),
        });
        let client = AutoAgentsChatClient::new(provider);

        // content as JSON object (not a string) — tests the other branch of the match
        let req = LlmCompleteRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": {"key": "value"}})],
            tools: vec![],
            system: None,
            max_tokens: 256,
        };

        let resp = client.complete(req).await.unwrap();
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }
}
