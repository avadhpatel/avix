pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicAdapter;
pub use openai::OpenAiAdapter;

use crate::error::AvixError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixToolCall {
    pub call_id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AvixToolResult {
    pub call_id: String,
    pub output: serde_json::Value,
    pub error: Option<String>,
}

pub trait ProviderAdapter: Send + Sync {
    fn provider_name(&self) -> &str;
    fn translate_tool(&self, descriptor: &serde_json::Value) -> serde_json::Value;
    fn parse_tool_call(&self, raw: &serde_json::Value) -> Result<AvixToolCall, AvixError>;
    fn format_tool_result(&self, result: &AvixToolResult) -> serde_json::Value;
}
