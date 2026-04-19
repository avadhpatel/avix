use crate::llm_client::{LlmCompleteResponse, StopReason};
use tracing::instrument;

use crate::llm_svc::adapter::AvixToolCall;
use crate::types::tool::ToolName;

pub enum TurnAction {
    ReturnResult(String),
    DispatchTools(Vec<AvixToolCall>),
    SummariseContext,
}

#[instrument(skip(resp))]
pub fn interpret_stop_reason(resp: &LlmCompleteResponse) -> TurnAction {
    match resp.stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => {
            let text = resp
                .content
                .iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            TurnAction::ReturnResult(text)
        }
        StopReason::ToolUse => {
            let calls = resp
                .content
                .iter()
                .filter(|c| c["type"] == "tool_use")
                .map(|c| {
                    let mangled = c["name"].as_str().unwrap_or("");
                    let name = ToolName::unmangle(mangled)
                        .map(|n| n.as_str().to_string())
                        .unwrap_or_else(|_| mangled.replace("__", "/"));
                    AvixToolCall {
                        call_id: c["id"].as_str().unwrap_or("").to_string(),
                        name,
                        args: c["input"].clone(),
                    }
                })
                .collect();
            TurnAction::DispatchTools(calls)
        }
        StopReason::MaxTokens => TurnAction::SummariseContext,
    }
}
