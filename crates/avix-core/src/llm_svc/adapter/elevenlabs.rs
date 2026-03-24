use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixEmbedRequest, AvixEmbedResponse,
    AvixImageRequest, AvixSpeechRequest, AvixToolCall, AvixToolDescriptor, AvixToolResult,
    AvixTranscribeRequest, AvixTranscribeResponse, ImageOutput, MultipartRequest, ProviderAdapter,
    SpeechEndpoint,
};
use crate::types::Modality;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct ElevenLabsAdapter;

impl ElevenLabsAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ElevenLabsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for ElevenLabsAdapter {
    fn provider_name(&self) -> &str {
        "elevenlabs"
    }

    fn modalities(&self) -> &[Modality] {
        &[Modality::Speech]
    }

    // Text methods return UnsupportedModality
    fn translate_tools(&self, _tools: &[AvixToolDescriptor]) -> Value {
        json!([])
    }

    fn build_complete_request(&self, _req: &AvixCompleteRequest) -> Value {
        json!({})
    }

    fn parse_complete_response(&self, _raw: Value) -> Result<AvixCompleteResponse, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Text))
    }

    fn parse_tool_call(&self, _raw: &Value) -> Result<AvixToolCall, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Text))
    }

    fn format_tool_result(&self, _result: &AvixToolResult) -> Value {
        json!({})
    }

    fn build_speech_request(&self, req: &AvixSpeechRequest) -> Result<Value, AdapterError> {
        Ok(json!({
            "text": req.text,
            "model_id": req.model,
            "voice_settings": {
                "stability": 0.5,
                "similarity_boost": 0.5,
            }
        }))
    }

    fn speech_endpoint(&self, req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        Ok(SpeechEndpoint {
            url: format!("/v1/text-to-speech/{}", req.voice),
            headers: HashMap::new(),
            format: req.format.clone().unwrap_or_else(|| "mp3".to_string()),
        })
    }

    // Image returns UnsupportedModality
    fn build_image_request(&self, _req: &AvixImageRequest) -> Result<Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    fn parse_image_response(&self, _raw: Value) -> Result<Vec<ImageOutput>, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    // Transcription returns UnsupportedModality
    fn build_transcription_request(
        &self,
        _req: &AvixTranscribeRequest,
        _audio_bytes: &[u8],
    ) -> Result<MultipartRequest, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    fn parse_transcription_response(
        &self,
        _raw: Value,
    ) -> Result<AvixTranscribeResponse, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Transcription))
    }

    // Embedding returns UnsupportedModality
    fn build_embed_request(&self, _req: &AvixEmbedRequest) -> Result<Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }

    fn parse_embed_response(&self, _raw: Value) -> Result<AvixEmbedResponse, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Embedding))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_adapter() -> ElevenLabsAdapter {
        ElevenLabsAdapter::new()
    }

    fn make_metadata() -> super::super::CompleteMetadata {
        super::super::CompleteMetadata {
            agent_pid: 1,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn test_provider_name() {
        let adapter = make_adapter();
        assert_eq!(adapter.provider_name(), "elevenlabs");
    }

    #[test]
    fn test_modalities_speech_only() {
        let adapter = make_adapter();
        let mods = adapter.modalities();
        assert_eq!(mods, &[Modality::Speech]);
    }

    #[test]
    fn test_build_speech_request() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
            provider: None,
            model: "eleven_multilingual_v2".to_string(),
            text: "Hello, world!".to_string(),
            voice: "rachel".to_string(),
            format: Some("mp3".to_string()),
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        let body = adapter.build_speech_request(&req).unwrap();
        assert_eq!(body["text"], "Hello, world!");
        assert_eq!(body["model_id"], "eleven_multilingual_v2");
        assert_eq!(body["voice_settings"]["stability"], 0.5);
        assert_eq!(body["voice_settings"]["similarity_boost"], 0.5);
    }

    #[test]
    fn test_speech_endpoint_includes_voice_in_url() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
            provider: None,
            model: "eleven_multilingual_v2".to_string(),
            text: "Hello".to_string(),
            voice: "rachel".to_string(),
            format: Some("mp3_44100_128".to_string()),
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        let endpoint = adapter.speech_endpoint(&req).unwrap();
        assert_eq!(endpoint.url, "/v1/text-to-speech/rachel");
        assert_eq!(endpoint.format, "mp3_44100_128");
    }

    #[test]
    fn test_speech_endpoint_default_format() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
            provider: None,
            model: "eleven_monolingual_v1".to_string(),
            text: "Hi".to_string(),
            voice: "adam".to_string(),
            format: None,
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        let endpoint = adapter.speech_endpoint(&req).unwrap();
        assert_eq!(endpoint.format, "mp3");
    }

    #[test]
    fn test_text_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_complete_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Text))
        ));
    }

    #[test]
    fn test_image_returns_unsupported() {
        let adapter = make_adapter();
        let req = AvixImageRequest {
            provider: None,
            model: "sdxl".to_string(),
            prompt: "cat".to_string(),
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
    fn test_embedding_returns_unsupported() {
        let adapter = make_adapter();
        let req = AvixEmbedRequest {
            provider: None,
            model: "embed".to_string(),
            input: super::super::EmbedInput::Single("hello".to_string()),
            metadata: make_metadata(),
        };
        assert!(matches!(
            adapter.build_embed_request(&req),
            Err(AdapterError::UnsupportedModality(Modality::Embedding))
        ));
    }

    #[test]
    fn test_parse_tool_call_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_tool_call(&json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Text))
        ));
    }

    #[test]
    fn test_parse_image_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_image_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Image))
        ));
    }

    #[test]
    fn test_parse_transcription_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_transcription_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Transcription))
        ));
    }

    #[test]
    fn test_parse_embed_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_embed_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Embedding))
        ));
    }

    #[test]
    fn test_translate_tools_returns_empty_array() {
        let adapter = make_adapter();
        let tools = vec![AvixToolDescriptor {
            name: "fs/read".to_string(),
            description: "read".to_string(),
            input_schema: json!({}),
        }];
        let result = adapter.translate_tools(&tools);
        assert_eq!(result, json!([]));
    }

    #[test]
    fn test_build_complete_request_returns_empty_object() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "m".to_string(),
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
        assert_eq!(body, json!({}));
    }

    #[test]
    fn test_format_tool_result_returns_empty_object() {
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "call-1".to_string(),
            output: json!({"content": "ok"}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert_eq!(formatted, json!({}));
    }

    #[test]
    fn test_default_impl() {
        let adapter = ElevenLabsAdapter;
        assert_eq!(adapter.provider_name(), "elevenlabs");
    }
}
