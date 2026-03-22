use super::{AvixToolCall, AvixToolResult, ProviderAdapter};
use crate::error::AvixError;
use crate::types::tool::ToolName;
use serde_json::{json, Value};

pub struct OpenAiAdapter;

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for OpenAiAdapter {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn translate_tool(&self, descriptor: &Value) -> Value {
        let name = descriptor["name"].as_str().unwrap_or("");
        let mangled = ToolName::parse(name)
            .map(|t| t.mangled())
            .unwrap_or_else(|_| name.replace('/', "__"));

        let input = descriptor.get("input").and_then(|v| v.as_object());
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        if let Some(fields) = input {
            for (field_name, field_def) in fields {
                let field_type = field_def["type"].as_str().unwrap_or("string");
                let json_type = match field_type {
                    "bool" => "boolean",
                    other => other,
                };
                properties.insert(field_name.clone(), json!({"type": json_type}));
                if field_def["required"].as_bool().unwrap_or(false) {
                    required.push(json!(field_name));
                }
            }
        }

        json!({
            "type": "function",
            "function": {
                "name": mangled,
                "description": descriptor["description"],
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required
                }
            }
        })
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AvixError> {
        let mangled_name = raw["function"]["name"].as_str().unwrap_or("");
        let name = ToolName::unmangle(mangled_name)
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|_| mangled_name.replace("__", "/"));
        let args_str = raw["function"]["arguments"].as_str().unwrap_or("{}");
        let args: serde_json::Value =
            serde_json::from_str(args_str).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(AvixToolCall {
            call_id: raw["id"].as_str().unwrap_or("").to_string(),
            name,
            args,
        })
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        json!({
            "role": "tool",
            "tool_call_id": result.call_id,
            "content": result.output.to_string()
        })
    }
}
