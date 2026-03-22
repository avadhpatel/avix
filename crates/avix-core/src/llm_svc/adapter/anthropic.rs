use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixToolCall, AvixToolDescriptor,
    AvixToolResult, ProviderAdapter, UsageSummary,
};
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
}
