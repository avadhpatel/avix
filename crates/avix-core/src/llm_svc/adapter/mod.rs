pub mod anthropic;
pub mod elevenlabs;
pub mod ollama;
pub mod openai;
pub mod stability;
pub mod xai;

pub use anthropic::AnthropicAdapter;
pub use elevenlabs::ElevenLabsAdapter;
pub use ollama::OllamaAdapter;
pub use openai::OpenAiAdapter;
pub use stability::StabilityAdapter;
pub use xai::XaiAdapter;

use crate::llm_client::StreamChunk;
use crate::types::Modality;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

// ── Core normalised types used by RuntimeExecutor ────────────────────────────

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

// ── Request / response types for each modality ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteMetadata {
    pub agent_pid: u32,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixCompleteRequest {
    pub provider: Option<String>,
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: Option<bool>,
    pub stop_sequences: Option<Vec<String>>,
    pub tools: Vec<AvixToolDescriptor>,
    pub metadata: CompleteMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixCompleteResponse {
    pub provider: String,
    pub model: String,
    pub content: Vec<serde_json::Value>,
    pub usage: UsageSummary,
    pub stop_reason: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixImageRequest {
    pub provider: Option<String>,
    pub model: String,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub size: Option<String>,
    pub style: Option<String>,
    pub n: Option<u32>,
    pub metadata: CompleteMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageOutput {
    pub file_path: String,
    pub mime_type: String,
    pub size: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixSpeechRequest {
    pub provider: Option<String>,
    pub model: String,
    pub text: String,
    pub voice: String,
    pub format: Option<String>,
    pub speed: Option<f32>,
    pub stream: Option<bool>,
    pub metadata: CompleteMetadata,
}

#[derive(Debug, Clone)]
pub struct SpeechEndpoint {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixTranscribeRequest {
    pub provider: Option<String>,
    pub model: String,
    pub file_path: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub granularity: Option<String>,
    pub metadata: CompleteMetadata,
}

#[derive(Debug, Clone)]
pub struct MultipartRequest {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub fields: HashMap<String, String>,
    pub audio_bytes: Vec<u8>,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeSegment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixTranscribeResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration_sec: Option<f32>,
    pub segments: Option<Vec<TranscribeSegment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EmbedInput {
    Single(String),
    Batch(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixEmbedRequest {
    pub provider: Option<String>,
    pub model: String,
    pub input: EmbedInput,
    pub metadata: CompleteMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedUsage {
    pub input_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvixEmbedResponse {
    pub embeddings: Vec<Vec<f32>>,
    pub dimensions: u32,
    pub usage: EmbedUsage,
}

// ── AdapterError ─────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("adapter does not support modality: {0:?}")]
    UnsupportedModality(Modality),
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("missing field: {0}")]
    MissingField(String),
}

// ── ProviderAdapter trait ─────────────────────────────────────────────────────

/// Pure translation layer — no I/O, no async.
/// Converts between Avix internal formats and provider wire JSON.
pub trait ProviderAdapter: Send + Sync {
    fn provider_name(&self) -> &str;
    fn modalities(&self) -> &[Modality];

    // ── Text (llm/complete) ──────────────────────────────────────────────────

    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> serde_json::Value;
    fn build_complete_request(&self, req: &AvixCompleteRequest) -> serde_json::Value;
    fn parse_complete_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixCompleteResponse, AdapterError>;
    fn parse_tool_call(&self, raw: &serde_json::Value) -> Result<AvixToolCall, AdapterError>;
    fn format_tool_result(&self, result: &AvixToolResult) -> serde_json::Value;

    // ── Streaming (llm/stream_complete) ─────────────────────────────────────

    /// HTTP path for the non-streaming text completion endpoint.
    ///
    /// OpenAI-compatible providers use `/v1/chat/completions`.
    /// Anthropic uses `/v1/messages`.
    fn complete_path(&self) -> &str {
        "/v1/chat/completions"
    }

    /// HTTP path for the **streaming** text completion endpoint.
    ///
    /// Defaults to the same path as `complete_path()`.
    /// Override when the streaming and non-streaming endpoints differ
    /// (e.g. OpenAI Responses API vs Chat Completions).
    fn stream_complete_path(&self) -> &str {
        self.complete_path()
    }

    /// Build the JSON request body for a **streaming** completion.
    ///
    /// Default: delegates to `build_complete_request()` and adds
    /// `"stream": true`.  Override for providers whose streaming
    /// endpoint accepts a different request shape.
    fn build_stream_request(&self, req: &AvixCompleteRequest) -> serde_json::Value {
        let mut body = self.build_complete_request(req);
        body["stream"] = serde_json::json!(true);
        body
    }

    /// Parse one SSE **data line** from a streaming completion response
    /// into a `StreamChunk`.
    ///
    /// `event_name` is the value of the preceding `event:` line (if any).
    /// `data` is the raw string after the `data: ` prefix.
    ///
    /// Returns:
    /// - `Ok(Some(chunk))` — a chunk to yield downstream.
    /// - `Ok(None)` — this data line should be skipped (e.g. a keepalive).
    /// - `Err(_)` — unrecoverable parse error.
    ///
    /// Default implementation returns an error; providers that support
    /// streaming must override this.
    fn parse_stream_event(
        &self,
        event_name: Option<&str>,
        data: &str,
    ) -> Result<Option<StreamChunk>, AdapterError> {
        let _ = (event_name, data);
        Err(AdapterError::UnsupportedModality(Modality::Text))
    }

    // ── Image (llm/generate-image) ───────────────────────────────────────────

    fn build_image_request(
        &self,
        _req: &AvixImageRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    fn parse_image_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<Vec<ImageOutput>, AdapterError> {
        let _ = raw;
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    // ── Speech (llm/generate-speech) ────────────────────────────────────────

    fn build_speech_request(
        &self,
        req: &AvixSpeechRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        let _ = req;
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    fn speech_endpoint(&self, req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        let _ = req;
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    // ── Transcription (llm/transcribe) ───────────────────────────────────────

    fn build_transcription_request(
        &self,
        req: &AvixTranscribeRequest,
        audio_bytes: &[u8],
    ) -> Result<MultipartRequest, AdapterError> {
        let _ = (req, audio_bytes);
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    fn parse_transcription_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixTranscribeResponse, AdapterError> {
        let _ = raw;
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    // ── Embedding (llm/embed) ────────────────────────────────────────────────

    fn build_embed_request(
        &self,
        req: &AvixEmbedRequest,
    ) -> Result<serde_json::Value, AdapterError> {
        let _ = req;
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }

    fn parse_embed_response(
        &self,
        raw: serde_json::Value,
    ) -> Result<AvixEmbedResponse, AdapterError> {
        let _ = raw;
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal adapter that only implements required text methods.
    struct TextOnlyAdapter;

    impl ProviderAdapter for TextOnlyAdapter {
        fn provider_name(&self) -> &str {
            "text-only"
        }

        fn modalities(&self) -> &[Modality] {
            &[Modality::Text]
        }

        fn translate_tools(&self, _tools: &[AvixToolDescriptor]) -> serde_json::Value {
            serde_json::json!([])
        }

        fn build_complete_request(&self, _req: &AvixCompleteRequest) -> serde_json::Value {
            serde_json::json!({})
        }

        fn parse_complete_response(
            &self,
            _raw: serde_json::Value,
        ) -> Result<AvixCompleteResponse, AdapterError> {
            Err(AdapterError::ParseError("not implemented".into()))
        }

        fn parse_tool_call(&self, _raw: &serde_json::Value) -> Result<AvixToolCall, AdapterError> {
            Err(AdapterError::ParseError("not implemented".into()))
        }

        fn format_tool_result(&self, _result: &AvixToolResult) -> serde_json::Value {
            serde_json::json!({})
        }
    }

    fn make_metadata() -> CompleteMetadata {
        CompleteMetadata {
            agent_pid: 1,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn test_default_build_image_request_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        let req = AvixImageRequest {
            provider: None,
            model: "m".into(),
            prompt: "p".into(),
            negative_prompt: None,
            size: None,
            style: None,
            n: None,
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.build_image_request(&req),
            Err(AdapterError::UnsupportedModality(Modality::Image))
        ));
    }

    #[test]
    fn test_default_parse_image_response_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        assert!(matches!(
            adapter.parse_image_response(serde_json::json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Image))
        ));
    }

    #[test]
    fn test_default_build_speech_request_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        let req = AvixSpeechRequest {
            provider: None,
            model: "tts".into(),
            text: "hello".into(),
            voice: "alloy".into(),
            format: None,
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.build_speech_request(&req),
            Err(AdapterError::UnsupportedModality(Modality::Speech))
        ));
    }

    #[test]
    fn test_default_speech_endpoint_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        let req = AvixSpeechRequest {
            provider: None,
            model: "tts".into(),
            text: "hello".into(),
            voice: "alloy".into(),
            format: None,
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.speech_endpoint(&req),
            Err(AdapterError::UnsupportedModality(Modality::Speech))
        ));
    }

    #[test]
    fn test_default_build_transcription_request_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        let req = AvixTranscribeRequest {
            provider: None,
            model: "whisper-1".into(),
            file_path: "/tmp/audio.wav".into(),
            language: None,
            prompt: None,
            granularity: None,
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.build_transcription_request(&req, &[]),
            Err(AdapterError::UnsupportedModality(Modality::Transcription))
        ));
    }

    #[test]
    fn test_default_parse_transcription_response_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        assert!(matches!(
            adapter.parse_transcription_response(serde_json::json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Transcription))
        ));
    }

    #[test]
    fn test_default_build_embed_request_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        let req = AvixEmbedRequest {
            provider: None,
            model: "embed".into(),
            input: EmbedInput::Single("hello".into()),
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.build_embed_request(&req),
            Err(AdapterError::UnsupportedModality(Modality::Embedding))
        ));
    }

    #[test]
    fn test_default_parse_embed_response_returns_unsupported() {
        let adapter = TextOnlyAdapter;
        assert!(matches!(
            adapter.parse_embed_response(serde_json::json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Embedding))
        ));
    }

    #[test]
    fn test_usage_summary_fields() {
        let usage = UsageSummary {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
        };
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_embed_input_single_serde_roundtrip() {
        let s = EmbedInput::Single("hello world".into());
        let v = serde_json::to_value(&s).unwrap();
        let back: EmbedInput = serde_json::from_value(v).unwrap();
        match back {
            EmbedInput::Single(text) => assert_eq!(text, "hello world"),
            _ => panic!("expected Single"),
        }
    }

    #[test]
    fn test_embed_input_batch_serde_roundtrip() {
        let b = EmbedInput::Batch(vec!["a".into(), "b".into(), "c".into()]);
        let v = serde_json::to_value(&b).unwrap();
        let back: EmbedInput = serde_json::from_value(v).unwrap();
        match back {
            EmbedInput::Batch(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], "a");
            }
            _ => panic!("expected Batch"),
        }
    }

    #[test]
    fn test_adapter_error_display() {
        let e1 = AdapterError::UnsupportedModality(Modality::Image);
        assert!(e1.to_string().contains("Image"));

        let e2 = AdapterError::ParseError("missing field x".into());
        assert!(e2.to_string().contains("missing field x"));

        let e3 = AdapterError::MissingField("prompt".into());
        assert!(e3.to_string().contains("prompt"));
    }

    #[test]
    fn test_avix_tool_call_fields() {
        let call = AvixToolCall {
            call_id: "id-123".into(),
            name: "fs/read".into(),
            args: serde_json::json!({"path": "/tmp/x"}),
        };
        assert_eq!(call.call_id, "id-123");
        assert_eq!(call.name, "fs/read");
        assert_eq!(call.args["path"], "/tmp/x");
    }

    #[test]
    fn test_avix_tool_result_fields() {
        let result = AvixToolResult {
            call_id: "id-456".into(),
            output: serde_json::json!({"content": "file contents"}),
            error: None,
        };
        assert_eq!(result.call_id, "id-456");
        assert!(result.error.is_none());

        let result_err = AvixToolResult {
            call_id: "id-789".into(),
            output: serde_json::json!(null),
            error: Some("EPERM".into()),
        };
        assert_eq!(result_err.error.as_deref(), Some("EPERM"));
    }

    #[test]
    fn test_complete_metadata_fields() {
        let meta = CompleteMetadata {
            agent_pid: 42,
            session_id: "sess-xyz".into(),
        };
        assert_eq!(meta.agent_pid, 42);
        assert_eq!(meta.session_id, "sess-xyz");
    }
}
