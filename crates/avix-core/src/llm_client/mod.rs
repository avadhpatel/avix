pub mod ipc_client;

pub use ipc_client::IpcLlmClient;

use crate::error::AvixError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

impl FromStr for StopReason {
    type Err = AvixError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "end_turn" => Ok(Self::EndTurn),
            "tool_use" => Ok(Self::ToolUse),
            "max_tokens" => Ok(Self::MaxTokens),
            "stop_sequence" => Ok(Self::StopSequence),
            other => Err(AvixError::ConfigParse(format!(
                "unknown stop reason: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCompleteRequest {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub tools: Vec<serde_json::Value>,
    pub system: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCompleteResponse {
    pub content: Vec<serde_json::Value>,
    pub stop_reason: StopReason,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl LlmCompleteResponse {
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse>;
}
