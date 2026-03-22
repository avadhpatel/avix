use super::{AvixToolCall, AvixToolResult, ProviderAdapter};
use crate::error::AvixError;
use crate::types::tool::ToolName;
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

    fn translate_tool(&self, descriptor: &Value) -> Value {
        let name = descriptor["name"].as_str().unwrap_or("");
        let mangled = ToolName::parse(name)
            .map(|t| t.mangled())
            .unwrap_or_else(|_| name.replace('/', "__"));

        // Build input_schema from "input" field
        let input = descriptor.get("input").and_then(|v| v.as_object());
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        if let Some(fields) = input {
            for (field_name, field_def) in fields {
                let field_type = field_def["type"].as_str().unwrap_or("string");
                let is_required = field_def["required"].as_bool().unwrap_or(false);
                let json_type = match field_type {
                    "bool" => "boolean",
                    other => other,
                };
                properties.insert(field_name.clone(), json!({"type": json_type}));
                if is_required {
                    required.push(json!(field_name));
                }
            }
        }

        json!({
            "name": mangled,
            "description": descriptor["description"],
            "input_schema": {
                "type": "object",
                "properties": properties,
                "required": required
            }
        })
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AvixError> {
        let mangled_name = raw["name"].as_str().unwrap_or("");
        let name = ToolName::unmangle(mangled_name)
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|_| mangled_name.replace("__", "/"));
        Ok(AvixToolCall {
            call_id: raw["id"].as_str().unwrap_or("").to_string(),
            name,
            args: raw["input"].clone(),
        })
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        let content = if let Some(err) = &result.error {
            json!([{
                "type": "tool_result",
                "tool_use_id": result.call_id,
                "content": err,
                "is_error": true
            }])
        } else {
            json!([{
                "type": "tool_result",
                "tool_use_id": result.call_id,
                "content": result.output.to_string()
            }])
        };
        json!({"role": "user", "content": content})
    }
}
