use super::openai::OpenAiAdapter;
use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixEmbedRequest, AvixEmbedResponse,
    AvixImageRequest, AvixSpeechRequest, AvixToolCall, AvixToolDescriptor, AvixToolResult,
    AvixTranscribeRequest, AvixTranscribeResponse, ImageOutput, MultipartRequest, ProviderAdapter,
    SpeechEndpoint,
};
use crate::types::Modality;
use serde_json::Value;

pub struct OllamaAdapter {
    inner: OpenAiAdapter,
}

impl OllamaAdapter {
    pub fn new() -> Self {
        Self {
            inner: OpenAiAdapter::new(),
        }
    }
}

impl Default for OllamaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for OllamaAdapter {
    fn provider_name(&self) -> &str {
        "ollama"
    }

    fn modalities(&self) -> &[Modality] {
        &[Modality::Text, Modality::Embedding]
    }

    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> Value {
        self.inner.translate_tools(tools)
    }

    fn build_complete_request(&self, req: &AvixCompleteRequest) -> Value {
        self.inner.build_complete_request(req)
    }

    fn parse_complete_response(&self, raw: Value) -> Result<AvixCompleteResponse, AdapterError> {
        self.inner.parse_complete_response(raw)
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AdapterError> {
        self.inner.parse_tool_call(raw)
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        self.inner.format_tool_result(result)
    }

    fn build_embed_request(&self, req: &AvixEmbedRequest) -> Result<Value, AdapterError> {
        self.inner.build_embed_request(req)
    }

    fn parse_embed_response(&self, raw: Value) -> Result<AvixEmbedResponse, AdapterError> {
        self.inner.parse_embed_response(raw)
    }

    // Override image/speech/transcription to return UnsupportedModality

    fn build_image_request(&self, _req: &AvixImageRequest) -> Result<Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    fn parse_image_response(&self, _raw: Value) -> Result<Vec<ImageOutput>, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Image))
    }

    fn build_speech_request(&self, _req: &AvixSpeechRequest) -> Result<Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    fn speech_endpoint(&self, _req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_adapter() -> OllamaAdapter {
        OllamaAdapter::new()
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
        assert_eq!(adapter.provider_name(), "ollama");
    }

    #[test]
    fn test_modalities_text_and_embedding_only() {
        let adapter = make_adapter();
        let mods = adapter.modalities();
        assert!(mods.contains(&Modality::Text));
        assert!(mods.contains(&Modality::Embedding));
        assert!(!mods.contains(&Modality::Image));
        assert!(!mods.contains(&Modality::Speech));
        assert!(!mods.contains(&Modality::Transcription));
    }

    #[test]
    fn test_translate_tools_delegates_to_openai() {
        let adapter = make_adapter();
        let tools = vec![super::super::AvixToolDescriptor {
            name: "fs/read".to_string(),
            description: "Read file".to_string(),
            input_schema: json!({}),
        }];
        let result = adapter.translate_tools(&tools);
        // Should produce OpenAI function format
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "fs__read");
    }

    #[test]
    fn test_build_complete_request_delegates() {
        let adapter = make_adapter();
        let req = super::super::AvixCompleteRequest {
            provider: None,
            model: "llama3".to_string(),
            messages: vec![json!({"role": "user", "content": "hi"})],
            system: None,
            max_tokens: None,
            temperature: None,
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_complete_request(&req);
        assert_eq!(body["model"], "llama3");
    }

    #[test]
    fn test_image_returns_unsupported() {
        let adapter = make_adapter();
        let req = super::super::AvixImageRequest {
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
    fn test_speech_returns_unsupported() {
        let adapter = make_adapter();
        let req = super::super::AvixSpeechRequest {
            provider: None,
            model: "tts".to_string(),
            text: "hello".to_string(),
            voice: "alloy".to_string(),
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
    fn test_transcription_returns_unsupported() {
        let adapter = make_adapter();
        let req = super::super::AvixTranscribeRequest {
            provider: None,
            model: "whisper".to_string(),
            file_path: "/tmp/audio.wav".to_string(),
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
    fn test_parse_image_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_image_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Image))
        ));
    }

    #[test]
    fn test_speech_endpoint_returns_unsupported() {
        let adapter = make_adapter();
        let req = super::super::AvixSpeechRequest {
            provider: None,
            model: "tts".to_string(),
            text: "hello".to_string(),
            voice: "alloy".to_string(),
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
    fn test_parse_transcription_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_transcription_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Transcription))
        ));
    }

    #[test]
    fn test_build_embed_delegates_to_openai() {
        use super::super::EmbedInput;
        let adapter = make_adapter();
        let req = super::super::AvixEmbedRequest {
            provider: None,
            model: "nomic-embed-text".to_string(),
            input: EmbedInput::Single("hello world".to_string()),
            metadata: make_metadata(),
        };
        let result = adapter.build_embed_request(&req);
        assert!(result.is_ok(), "embed should work for ollama");
        let body = result.unwrap();
        assert_eq!(body["model"], "nomic-embed-text");
    }

    #[test]
    fn test_format_tool_result_delegates() {
        let adapter = make_adapter();
        let result = super::super::AvixToolResult {
            call_id: "call-1".to_string(),
            output: json!({"content": "ok"}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert!(formatted.is_object());
    }

    #[test]
    fn test_parse_tool_call_delegates() {
        let adapter = make_adapter();
        let raw = json!({
            "id": "call-1",
            "type": "function",
            "function": {
                "name": "fs__read",
                "arguments": "{\"path\":\"/tmp/x\"}"
            }
        });
        let result = adapter.parse_tool_call(&raw);
        // Delegates to OpenAI adapter parsing
        assert!(result.is_ok(), "parse_tool_call should succeed: {result:?}");
        let call = result.unwrap();
        assert_eq!(call.name, "fs/read"); // unmangled
    }

    #[test]
    fn test_default_impl() {
        let adapter = OllamaAdapter::default();
        assert_eq!(adapter.provider_name(), "ollama");
    }
}
