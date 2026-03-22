use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixImageRequest, AvixSpeechRequest,
    AvixToolCall, AvixToolDescriptor, AvixToolResult, AvixTranscribeRequest,
    AvixTranscribeResponse, ImageOutput, MultipartRequest, ProviderAdapter, SpeechEndpoint,
};
use crate::types::Modality;
use base64::Engine as _;
use serde_json::{json, Value};

pub struct StabilityAdapter;

impl StabilityAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StabilityAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for StabilityAdapter {
    fn provider_name(&self) -> &str {
        "stability-ai"
    }

    fn modalities(&self) -> &[Modality] {
        &[Modality::Image]
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

    fn build_image_request(&self, req: &AvixImageRequest) -> Result<Value, AdapterError> {
        let mut body = json!({
            "prompt": req.prompt,
            "output_format": "png",
            "aspect_ratio": req.size.as_deref().unwrap_or("1:1"),
        });

        if let Some(neg) = &req.negative_prompt {
            body["negative_prompt"] = json!(neg);
        }

        Ok(body)
    }

    fn parse_image_response(&self, raw: Value) -> Result<Vec<ImageOutput>, AdapterError> {
        let b64 = raw["image"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("image".to_string()))?;

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| AdapterError::ParseError(e.to_string()))?;

        let bytes_len = decoded.len() as u64;

        Ok(vec![ImageOutput {
            file_path: String::new(),
            mime_type: "image/png".to_string(),
            size: None,
            bytes: bytes_len,
        }])
    }

    // Speech returns UnsupportedModality
    fn build_speech_request(&self, _req: &AvixSpeechRequest) -> Result<Value, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
    }

    fn speech_endpoint(&self, _req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        Err(AdapterError::UnsupportedModality(Modality::Speech))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_adapter() -> StabilityAdapter {
        StabilityAdapter::new()
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
        assert_eq!(adapter.provider_name(), "stability-ai");
    }

    #[test]
    fn test_modalities_image_only() {
        let adapter = make_adapter();
        let mods = adapter.modalities();
        assert_eq!(mods, &[Modality::Image]);
    }

    #[test]
    fn test_build_image_request() {
        let adapter = make_adapter();
        let req = AvixImageRequest {
            provider: None,
            model: "sd3".to_string(),
            prompt: "A majestic cat".to_string(),
            negative_prompt: Some("blurry".to_string()),
            size: Some("16:9".to_string()),
            style: None,
            n: None,
            metadata: make_metadata(),
        };
        let body = adapter.build_image_request(&req).unwrap();
        assert_eq!(body["prompt"], "A majestic cat");
        assert_eq!(body["output_format"], "png");
        assert_eq!(body["aspect_ratio"], "16:9");
        assert_eq!(body["negative_prompt"], "blurry");
    }

    #[test]
    fn test_build_image_request_default_aspect_ratio() {
        let adapter = make_adapter();
        let req = AvixImageRequest {
            provider: None,
            model: "sd3".to_string(),
            prompt: "cat".to_string(),
            negative_prompt: None,
            size: None,
            style: None,
            n: None,
            metadata: make_metadata(),
        };
        let body = adapter.build_image_request(&req).unwrap();
        assert_eq!(body["aspect_ratio"], "1:1");
        // No negative_prompt when not provided
        assert!(body.get("negative_prompt").is_none());
    }

    #[test]
    fn test_parse_image_response() {
        let adapter = make_adapter();
        let fake_bytes = vec![0u8; 200];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fake_bytes);
        let raw = json!({
            "image": b64,
            "finish_reason": "SUCCESS",
            "seed": 12345
        });
        let outputs = adapter.parse_image_response(raw).unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].mime_type, "image/png");
        assert_eq!(outputs[0].bytes, 200);
        assert!(outputs[0].file_path.is_empty());
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
    fn test_speech_returns_unsupported() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
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
    fn test_speech_endpoint_returns_unsupported() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
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
    fn test_transcription_returns_unsupported() {
        let adapter = make_adapter();
        let req = AvixTranscribeRequest {
            provider: None,
            model: "whisper-1".to_string(),
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
    fn test_parse_transcription_response_returns_unsupported() {
        let adapter = make_adapter();
        assert!(matches!(
            adapter.parse_transcription_response(json!({})),
            Err(AdapterError::UnsupportedModality(Modality::Transcription))
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
    fn test_translate_tools_returns_empty_array() {
        let adapter = make_adapter();
        let result = adapter.translate_tools(&[]);
        assert_eq!(result, json!([]));
    }

    #[test]
    fn test_build_complete_request_returns_empty_object() {
        use super::super::AvixCompleteRequest;
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "sd3".to_string(),
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
        use super::super::AvixToolResult;
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "c1".to_string(),
            output: json!({}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert_eq!(formatted, json!({}));
    }

    #[test]
    fn test_parse_image_response_missing_image_field() {
        let adapter = make_adapter();
        // Missing "image" field → MissingField error
        let raw = json!({"finish_reason": "SUCCESS"});
        assert!(matches!(
            adapter.parse_image_response(raw),
            Err(AdapterError::MissingField(_))
        ));
    }

    #[test]
    fn test_parse_image_response_invalid_base64() {
        let adapter = make_adapter();
        let raw = json!({"image": "not-valid-base64!!!"});
        assert!(matches!(
            adapter.parse_image_response(raw),
            Err(AdapterError::ParseError(_))
        ));
    }

    #[test]
    fn test_default_impl() {
        let adapter = StabilityAdapter::default();
        assert_eq!(adapter.provider_name(), "stability-ai");
    }
}
