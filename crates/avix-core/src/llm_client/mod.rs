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
pub struct LlmCompleteRequest {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub tools: Vec<serde_json::Value>,
    pub system: Option<String>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reason_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_value(StopReason::EndTurn).unwrap(),
            serde_json::json!("end_turn")
        );
        assert_eq!(
            serde_json::to_value(StopReason::ToolUse).unwrap(),
            serde_json::json!("tool_use")
        );
        assert_eq!(
            serde_json::to_value(StopReason::MaxTokens).unwrap(),
            serde_json::json!("max_tokens")
        );
        assert_eq!(
            serde_json::to_value(StopReason::StopSequence).unwrap(),
            serde_json::json!("stop_sequence")
        );
    }

    #[test]
    fn stop_reason_round_trips() {
        for variant in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::MaxTokens,
            StopReason::StopSequence,
        ] {
            let json = serde_json::to_value(&variant).unwrap();
            let back: StopReason = serde_json::from_value(json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn llm_complete_request_uses_snake_case_on_wire() {
        let req = LlmCompleteRequest {
            model: "m".into(),
            messages: vec![],
            tools: vec![],
            system: None,
            max_tokens: 512,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("max_tokens").is_some(), "expected snake_case field max_tokens");
        assert!(v.get("maxTokens").is_none(), "must not produce camelCase maxTokens");
    }

    #[test]
    fn llm_complete_response_uses_snake_case_on_wire() {
        let resp = LlmCompleteResponse {
            content: vec![],
            stop_reason: StopReason::EndTurn,
            input_tokens: 10,
            output_tokens: 20,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["input_tokens"], 10);
        assert_eq!(v["output_tokens"], 20);
        assert!(v.get("stopReason").is_none(), "must not produce camelCase stopReason");
        assert!(v.get("inputTokens").is_none());
        assert!(v.get("outputTokens").is_none());
    }

    #[test]
    fn llm_complete_response_deserializes_from_snake_case() {
        let json = serde_json::json!({
            "content": [],
            "stop_reason": "tool_use",
            "input_tokens": 5,
            "output_tokens": 15,
        });
        let resp: LlmCompleteResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        assert_eq!(resp.input_tokens, 5);
        assert_eq!(resp.output_tokens, 15);
    }
}
