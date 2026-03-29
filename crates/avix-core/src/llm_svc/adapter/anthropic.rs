use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixToolCall, AvixToolDescriptor,
    AvixToolResult, ProviderAdapter, UsageSummary,
};
use crate::llm_client::{StopReason, StreamChunk};
use crate::types::{tool::ToolName, Modality};
use serde_json::{json, Value};

pub struct AnthropicAdapter;

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AnthropicAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for AnthropicAdapter {
    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn modalities(&self) -> &[Modality] {
        &[Modality::Text]
    }

    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> Value {
        let translated: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mangled = ToolName::parse(&t.name)
                    .map(|tn| tn.mangled())
                    .unwrap_or_else(|_| t.name.replace('/', "__"));
                json!({
                    "name": mangled,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        json!(translated)
    }

    fn build_complete_request(&self, req: &AvixCompleteRequest) -> Value {
        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "messages": req.messages,
        });

        if let Some(sys) = &req.system {
            body["system"] = json!(sys);
        }

        let tools = self.translate_tools(&req.tools);
        if let Some(arr) = tools.as_array() {
            if !arr.is_empty() {
                body["tools"] = tools;
            }
        }

        body
    }

    fn parse_complete_response(&self, raw: Value) -> Result<AvixCompleteResponse, AdapterError> {
        let content = raw["content"].as_array().cloned().unwrap_or_default();

        let stop_reason = raw["stop_reason"]
            .as_str()
            .unwrap_or("end_turn")
            .to_string();

        let input_tokens = raw["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = raw["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;

        Ok(AvixCompleteResponse {
            provider: "anthropic".to_string(),
            model: raw["model"].as_str().unwrap_or("").to_string(),
            content,
            usage: UsageSummary {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            },
            stop_reason,
            latency_ms: 0,
        })
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AdapterError> {
        let call_id = raw["id"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("id".to_string()))?
            .to_string();

        let mangled_name = raw["name"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("name".to_string()))?;

        let name = ToolName::unmangle(mangled_name)
            .map(|tn| tn.as_str().to_string())
            .unwrap_or_else(|_| mangled_name.replace("__", "/"));

        let args = raw["input"].clone();

        Ok(AvixToolCall {
            call_id,
            name,
            args,
        })
    }

    fn complete_path(&self) -> &str {
        "/v1/messages"
    }

    fn build_stream_request(&self, req: &AvixCompleteRequest) -> Value {
        let mut body = self.build_complete_request(req);
        body["stream"] = json!(true);
        body
    }

    /// Parse one Anthropic Messages API SSE event into a `StreamChunk`.
    ///
    /// Anthropic SSE events have an explicit `event:` line followed by a
    /// `data:` line containing JSON.  The relevant events are:
    ///
    /// | event                 | action                            |
    /// |-----------------------|-----------------------------------|
    /// | `content_block_start` | text block → nothing; tool_use → `ToolCallStart` |
    /// | `content_block_delta` | `text_delta` → `TextDelta`; `input_json_delta` → `ToolCallArgsDelta` |
    /// | `content_block_stop`  | tool block → `ToolCallComplete`   |
    /// | `message_delta`       | `Done` with stop_reason + usage   |
    /// | everything else       | skipped                           |
    fn parse_stream_event(
        &self,
        event_name: Option<&str>,
        data: &str,
    ) -> Result<Option<StreamChunk>, AdapterError> {
        let v: Value =
            serde_json::from_str(data).map_err(|e| AdapterError::ParseError(e.to_string()))?;

        match event_name {
            Some("content_block_start") => {
                let block = &v["content_block"];
                if block["type"].as_str() == Some("tool_use") {
                    let call_id = block["id"].as_str().unwrap_or("").to_string();
                    let mangled = block["name"].as_str().unwrap_or("");
                    let name = ToolName::unmangle(mangled)
                        .map(|tn| tn.as_str().to_string())
                        .unwrap_or_else(|_| mangled.replace("__", "/"));
                    return Ok(Some(StreamChunk::ToolCallStart { call_id, name }));
                }
                Ok(None)
            }
            Some("content_block_delta") => {
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let text = delta["text"].as_str().unwrap_or("").to_string();
                        if text.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(StreamChunk::TextDelta { text }))
                        }
                    }
                    Some("input_json_delta") => {
                        // The index lets us identify which tool call this belongs to,
                        // but Anthropic sends tool calls sequentially so we use the
                        // index to look up the call_id (stored upstream).
                        // We surface the delta with a sentinel call_id; the executor
                        // resolves it by index.
                        let index = v["index"].as_u64().unwrap_or(0).to_string();
                        let args_delta = delta["partial_json"].as_str().unwrap_or("").to_string();
                        if args_delta.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(StreamChunk::ToolCallArgsDelta {
                                call_id: index,
                                args_delta,
                            }))
                        }
                    }
                    _ => Ok(None),
                }
            }
            Some("content_block_stop") => {
                // Only meaningful for tool_use blocks.  We emit ToolCallComplete
                // keyed by the block index; the executor resolves the call_id.
                let index = v["index"].as_u64().unwrap_or(0).to_string();
                Ok(Some(StreamChunk::ToolCallComplete { call_id: index }))
            }
            Some("message_delta") => {
                let delta = &v["delta"];
                let stop_reason = match delta["stop_reason"].as_str().unwrap_or("end_turn") {
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    "stop_sequence" => StopReason::StopSequence,
                    _ => StopReason::EndTurn,
                };
                let input_tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
                let output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
                Ok(Some(StreamChunk::Done {
                    stop_reason,
                    input_tokens,
                    output_tokens,
                }))
            }
            _ => Ok(None),
        }
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        let mut content_block = json!({
            "type": "tool_result",
            "tool_use_id": result.call_id,
            "content": result.output.to_string(),
        });

        if result.error.is_some() {
            content_block["is_error"] = json!(true);
        }

        json!({
            "role": "user",
            "content": [content_block],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::StreamChunk;
    use serde_json::json;

    fn make_adapter() -> AnthropicAdapter {
        AnthropicAdapter::new()
    }

    fn make_metadata() -> super::super::CompleteMetadata {
        super::super::CompleteMetadata {
            agent_pid: 1,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn test_translate_tools_mangles_name() {
        let adapter = make_adapter();
        let tools = vec![AvixToolDescriptor {
            name: "fs/read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object"}),
        }];
        let result = adapter.translate_tools(&tools);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "fs__read");
    }

    #[test]
    fn test_build_complete_request_has_model() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "claude-3-opus-20240229".to_string(),
            messages: vec![json!({"role": "user", "content": "hello"})],
            system: Some("You are helpful.".to_string()),
            max_tokens: Some(1024),
            temperature: None,
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_complete_request(&req);
        assert_eq!(body["model"], "claude-3-opus-20240229");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "You are helpful.");
    }

    #[test]
    fn test_parse_complete_response() {
        let adapter = make_adapter();
        let raw = json!({
            "model": "claude-3-opus-20240229",
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        });
        let resp = adapter.parse_complete_response(raw).unwrap();
        assert_eq!(resp.provider, "anthropic");
        assert_eq!(resp.stop_reason, "end_turn");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert_eq!(resp.usage.total_tokens, 15);
        assert_eq!(resp.content.len(), 1);
    }

    #[test]
    fn test_parse_tool_call_unmangles() {
        let adapter = make_adapter();
        let raw = json!({
            "type": "tool_use",
            "id": "toolu_abc",
            "name": "fs__write",
            "input": {"path": "/tmp/file", "content": "data"}
        });
        let call = adapter.parse_tool_call(&raw).unwrap();
        assert_eq!(call.call_id, "toolu_abc");
        assert_eq!(call.name, "fs/write");
        assert_eq!(call.args["path"], "/tmp/file");
    }

    #[test]
    fn test_format_tool_result_success() {
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "toolu_abc".to_string(),
            output: json!({"status": "ok"}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert_eq!(formatted["role"], "user");
        let content = &formatted["content"][0];
        assert_eq!(content["type"], "tool_result");
        assert_eq!(content["tool_use_id"], "toolu_abc");
        assert!(content.get("is_error").is_none());
    }

    #[test]
    fn test_format_tool_result_error() {
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "toolu_xyz".to_string(),
            output: json!(null),
            error: Some("EPERM".to_string()),
        };
        let formatted = adapter.format_tool_result(&result);
        let content = &formatted["content"][0];
        assert_eq!(content["is_error"], true);
        assert_eq!(content["tool_use_id"], "toolu_xyz");
    }

    #[test]
    fn test_build_complete_request_no_tools_omits_tools_field() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "claude-3-haiku-20240307".to_string(),
            messages: vec![],
            system: None,
            max_tokens: None,
            temperature: None,
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_complete_request(&req);
        // tools field should not be present when there are no tools
        assert!(body.get("tools").is_none());
        // max_tokens defaults to 4096
        assert_eq!(body["max_tokens"], 4096);
    }

    #[test]
    fn test_complete_path_is_messages() {
        let adapter = make_adapter();
        assert_eq!(adapter.complete_path(), "/v1/messages");
        assert_eq!(adapter.stream_complete_path(), "/v1/messages");
    }

    #[test]
    fn test_build_stream_request_adds_stream_true() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "claude-3-haiku-20240307".into(),
            messages: vec![json!({"role": "user", "content": "hi"})],
            system: None,
            max_tokens: Some(256),
            temperature: None,
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_stream_request(&req);
        assert_eq!(body["stream"], true);
        assert_eq!(body["model"], "claude-3-haiku-20240307");
    }

    #[test]
    fn test_parse_stream_event_text_delta() {
        let adapter = make_adapter();
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let chunk = adapter
            .parse_stream_event(Some("content_block_delta"), data)
            .unwrap();
        assert!(matches!(chunk, Some(StreamChunk::TextDelta { text }) if text == "Hello"));
    }

    #[test]
    fn test_parse_stream_event_tool_call_start() {
        let adapter = make_adapter();
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"fs__read","input":{}}}"#;
        let chunk = adapter
            .parse_stream_event(Some("content_block_start"), data)
            .unwrap();
        assert!(
            matches!(&chunk, Some(StreamChunk::ToolCallStart { call_id, name }) if call_id == "toolu_abc" && name == "fs/read")
        );
    }

    #[test]
    fn test_parse_stream_event_tool_args_delta() {
        let adapter = make_adapter();
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
        let chunk = adapter
            .parse_stream_event(Some("content_block_delta"), data)
            .unwrap();
        assert!(
            matches!(&chunk, Some(StreamChunk::ToolCallArgsDelta { call_id, args_delta }) if call_id == "1" && args_delta.contains("path"))
        );
    }

    #[test]
    fn test_parse_stream_event_done() {
        let adapter = make_adapter();
        let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#;
        let chunk = adapter
            .parse_stream_event(Some("message_delta"), data)
            .unwrap();
        assert!(
            matches!(chunk, Some(StreamChunk::Done { stop_reason, output_tokens, .. }) if stop_reason == StopReason::EndTurn && output_tokens == 42)
        );
    }

    #[test]
    fn test_parse_stream_event_unknown_event_returns_none() {
        let adapter = make_adapter();
        let data = r#"{"type":"ping"}"#;
        let chunk = adapter.parse_stream_event(Some("ping"), data).unwrap();
        assert!(chunk.is_none());
    }
}
