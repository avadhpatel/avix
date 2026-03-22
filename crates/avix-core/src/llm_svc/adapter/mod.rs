pub mod anthropic;
pub mod elevenlabs;
pub mod ollama;
pub mod openai;
pub mod stability;

pub use anthropic::AnthropicAdapter;
pub use elevenlabs::ElevenLabsAdapter;
pub use ollama::OllamaAdapter;
pub use openai::OpenAiAdapter;
pub use stability::StabilityAdapter;

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
