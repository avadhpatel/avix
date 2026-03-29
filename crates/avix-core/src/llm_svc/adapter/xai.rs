use super::openai::OpenAiAdapter;
use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixToolCall, AvixToolDescriptor,
    AvixToolResult, ProviderAdapter,
};
use crate::llm_client::StreamChunk;
use crate::types::Modality;
use serde_json::Value;

/// xAI (Grok) adapter.
///
/// xAI exposes an OpenAI-compatible API at `https://api.x.ai/v1`, so all
/// wire-format translation delegates to [`OpenAiAdapter`]. The only difference
/// is the provider name returned in responses and the modality set (text only).
pub struct XaiAdapter {
    inner: OpenAiAdapter,
}

impl XaiAdapter {
    pub fn new() -> Self {
        Self {
            inner: OpenAiAdapter::new(),
        }
    }
}

impl Default for XaiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for XaiAdapter {
    fn provider_name(&self) -> &str {
        "xai"
    }

    fn modalities(&self) -> &[Modality] {
        &[Modality::Text]
    }

    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> Value {
        self.inner.translate_tools(tools)
    }

    fn build_complete_request(&self, req: &AvixCompleteRequest) -> Value {
        self.inner.build_complete_request(req)
    }

    fn parse_complete_response(&self, raw: Value) -> Result<AvixCompleteResponse, AdapterError> {
        let mut resp = self.inner.parse_complete_response(raw)?;
        resp.provider = "xai".to_string();
        Ok(resp)
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AdapterError> {
        self.inner.parse_tool_call(raw)
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        self.inner.format_tool_result(result)
    }

    fn parse_stream_event(
        &self,
        event_name: Option<&str>,
        data: &str,
    ) -> Result<Option<StreamChunk>, AdapterError> {
        self.inner.parse_stream_event(event_name, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_adapter() -> XaiAdapter {
        XaiAdapter::new()
    }

    fn make_metadata() -> super::super::CompleteMetadata {
        super::super::CompleteMetadata {
            agent_pid: 1,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn test_provider_name() {
        assert_eq!(make_adapter().provider_name(), "xai");
    }

    #[test]
    fn test_modalities_text_only() {
        let adapter = make_adapter();
        let mods = adapter.modalities();
        assert_eq!(mods, &[Modality::Text]);
    }

    #[test]
    fn test_translate_tools_mangles_names() {
        let adapter = make_adapter();
        let tools = vec![AvixToolDescriptor {
            name: "fs/read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        let result = adapter.translate_tools(&tools);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["function"]["name"], "fs__read");
    }

    #[test]
    fn test_build_complete_request_has_model() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "grok-3".to_string(),
            messages: vec![json!({"role": "user", "content": "hello"})],
            system: None,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_complete_request(&req);
        assert_eq!(body["model"], "grok-3");
        assert_eq!(body["max_tokens"], 1024);
    }

    #[test]
    fn test_parse_complete_response_sets_xai_provider() {
        let adapter = make_adapter();
        let raw = json!({
            "model": "grok-3",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello from Grok!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8,
                "total_tokens": 18
            }
        });
        let resp = adapter.parse_complete_response(raw).unwrap();
        assert_eq!(resp.provider, "xai");
        assert_eq!(resp.stop_reason, "stop");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 8);
        assert_eq!(resp.content[0]["text"], "Hello from Grok!");
    }

    #[test]
    fn test_parse_complete_response_tool_call() {
        let adapter = make_adapter();
        let raw = json!({
            "model": "grok-3",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "fs__read",
                            "arguments": "{\"path\": \"/tmp/file\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 10
            }
        });
        let resp = adapter.parse_complete_response(raw).unwrap();
        assert_eq!(resp.provider, "xai");
        assert_eq!(resp.stop_reason, "tool_use");
        assert_eq!(resp.content[0]["name"], "fs/read");
    }

    #[test]
    fn test_parse_tool_call_unmangles() {
        let adapter = make_adapter();
        let raw = json!({
            "id": "call_xyz",
            "type": "function",
            "function": {
                "name": "mem__store",
                "arguments": "{\"key\": \"fact\", \"value\": \"42\"}"
            }
        });
        let call = adapter.parse_tool_call(&raw).unwrap();
        assert_eq!(call.call_id, "call_xyz");
        assert_eq!(call.name, "mem/store");
    }

    #[test]
    fn test_format_tool_result() {
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "call_abc".to_string(),
            output: json!({"status": "ok"}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert_eq!(formatted["role"], "tool");
        assert_eq!(formatted["tool_call_id"], "call_abc");
    }

    #[test]
    fn test_image_modality_unsupported() {
        use super::super::{AvixImageRequest, CompleteMetadata};
        let adapter = make_adapter();
        let req = AvixImageRequest {
            provider: None,
            model: "grok-2-vision".to_string(),
            prompt: "a cat".to_string(),
            negative_prompt: None,
            size: None,
            style: None,
            n: None,
            metadata: CompleteMetadata {
                agent_pid: 1,
                session_id: "s".to_string(),
            },
        };
        assert!(matches!(
            adapter.build_image_request(&req),
            Err(AdapterError::UnsupportedModality(Modality::Image))
        ));
    }
}
