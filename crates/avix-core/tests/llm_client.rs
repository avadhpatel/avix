use async_trait::async_trait;
use avix_core::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse, StopReason};
use serde_json::json;
use uuid::Uuid;

struct MockLlmClient {
    response: LlmCompleteResponse,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn llm_client_complete_returns_response() {
    let client = MockLlmClient {
        response: LlmCompleteResponse {
            content: vec![json!({"type": "text", "text": "Hello, world!"})],
            stop_reason: StopReason::EndTurn,
            input_tokens: 10,
            output_tokens: 5,
        },
    };
    let req = LlmCompleteRequest {
        model: "claude-sonnet-4".into(),
        messages: vec![json!({"role": "user", "content": "Hi"})],
        tools: vec![],
        system: None,
        max_tokens: 1000,
        turn_id: Uuid::nil(),
    };
    let resp = client.complete(req).await.unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(resp.input_tokens, 10);
}

#[test]
fn stop_reason_from_str() {
    assert_eq!(
        "end_turn".parse::<StopReason>().unwrap(),
        StopReason::EndTurn
    );
    assert_eq!(
        "tool_use".parse::<StopReason>().unwrap(),
        StopReason::ToolUse
    );
    assert_eq!(
        "max_tokens".parse::<StopReason>().unwrap(),
        StopReason::MaxTokens
    );
    assert_eq!(
        "stop_sequence".parse::<StopReason>().unwrap(),
        StopReason::StopSequence
    );
    assert!("unknown".parse::<StopReason>().is_err());
}

#[test]
fn llm_complete_request_serialises() {
    let req = LlmCompleteRequest {
        model: "claude-opus-4".into(),
        messages: vec![json!({"role": "user", "content": "Hello"})],
        tools: vec![json!({"name": "fs__read"})],
        system: Some("You are a researcher.".into()),
        max_tokens: 2000,
        turn_id: Uuid::nil(),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["model"], "claude-opus-4");
    assert_eq!(v["max_tokens"], 2000);
    assert!(v["tools"].as_array().unwrap().len() == 1);
}

#[test]
fn response_total_tokens() {
    let resp = LlmCompleteResponse {
        content: vec![],
        stop_reason: StopReason::EndTurn,
        input_tokens: 100,
        output_tokens: 50,
    };
    assert_eq!(resp.total_tokens(), 150);
}
