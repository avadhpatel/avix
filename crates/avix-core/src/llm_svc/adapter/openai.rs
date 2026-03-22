use super::{
    AdapterError, AvixCompleteRequest, AvixCompleteResponse, AvixEmbedRequest, AvixEmbedResponse,
    AvixImageRequest, AvixSpeechRequest, AvixToolCall, AvixToolDescriptor, AvixToolResult,
    AvixTranscribeRequest, AvixTranscribeResponse, EmbedInput, EmbedUsage, ImageOutput,
    MultipartRequest, ProviderAdapter, SpeechEndpoint, TranscribeSegment, UsageSummary,
};
use crate::types::{tool::ToolName, Modality};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct OpenAiAdapter;

impl OpenAiAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for OpenAiAdapter {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn modalities(&self) -> &[Modality] {
        &[
            Modality::Text,
            Modality::Image,
            Modality::Speech,
            Modality::Transcription,
            Modality::Embedding,
        ]
    }

    fn translate_tools(&self, tools: &[AvixToolDescriptor]) -> Value {
        let translated: Vec<Value> = tools
            .iter()
            .map(|t| {
                let mangled = ToolName::parse(&t.name)
                    .map(|tn| tn.mangled())
                    .unwrap_or_else(|_| t.name.replace('/', "__"));
                json!({
                    "type": "function",
                    "function": {
                        "name": mangled,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        json!(translated)
    }

    fn build_complete_request(&self, req: &AvixCompleteRequest) -> Value {
        let mut body = json!({
            "model": req.model,
            "messages": req.messages,
        });

        if let Some(max_tokens) = req.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        if let Some(temperature) = req.temperature {
            body["temperature"] = json!(temperature);
        }

        if let Some(stop) = &req.stop_sequences {
            if !stop.is_empty() {
                body["stop"] = json!(stop);
            }
        }

        let tools = self.translate_tools(&req.tools);
        if let Some(arr) = tools.as_array() {
            if !arr.is_empty() {
                body["tools"] = tools;
            }
        }

        body
    }

    fn parse_complete_response(&self, raw: Value) -> Result<AvixCompleteResponse, AdapterError> {
        let choice = raw["choices"]
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| AdapterError::MissingField("choices[0]".to_string()))?;

        let message = &choice["message"];
        let mut content: Vec<Value> = Vec::new();

        if let Some(text) = message["content"].as_str() {
            if !text.is_empty() {
                content.push(json!({"type": "text", "text": text}));
            }
        }

        let stop_reason = if message
            .get("tool_calls")
            .and_then(|tc| tc.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            if let Some(tool_calls) = message["tool_calls"].as_array() {
                for tc in tool_calls {
                    let id = tc["id"].as_str().unwrap_or("").to_string();
                    let mangled_name = tc["function"]["name"].as_str().unwrap_or("");
                    let name = ToolName::unmangle(mangled_name)
                        .map(|tn| tn.as_str().to_string())
                        .unwrap_or_else(|_| mangled_name.replace("__", "/"));
                    let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                    let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                    content.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": args,
                    }));
                }
            }
            "tool_use".to_string()
        } else {
            choice["finish_reason"]
                .as_str()
                .unwrap_or("stop")
                .to_string()
        };

        let input_tokens = raw["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
        let output_tokens = raw["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;

        Ok(AvixCompleteResponse {
            provider: "openai".to_string(),
            model: raw["model"].as_str().unwrap_or("").to_string(),
            content,
            usage: UsageSummary {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
            },
            stop_reason,
            latency_ms: 0,
        })
    }

    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AdapterError> {
        let call_id = raw["id"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("id".to_string()))?
            .to_string();

        let mangled_name = raw["function"]["name"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("function.name".to_string()))?;

        let name = ToolName::unmangle(mangled_name)
            .map(|tn| tn.as_str().to_string())
            .unwrap_or_else(|_| mangled_name.replace("__", "/"));

        let args_str = raw["function"]["arguments"].as_str().unwrap_or("{}");
        let args: Value =
            serde_json::from_str(args_str).map_err(|e| AdapterError::ParseError(e.to_string()))?;

        Ok(AvixToolCall {
            call_id,
            name,
            args,
        })
    }

    fn format_tool_result(&self, result: &AvixToolResult) -> Value {
        let output_str = match &result.error {
            Some(err) => format!("Error: {err}"),
            None => result.output.to_string(),
        };

        json!({
            "role": "tool",
            "tool_call_id": result.call_id,
            "content": output_str,
        })
    }

    fn build_image_request(&self, req: &AvixImageRequest) -> Result<Value, AdapterError> {
        Ok(json!({
            "model": req.model,
            "prompt": req.prompt,
            "n": req.n.unwrap_or(1),
            "size": req.size.as_deref().unwrap_or("1024x1024"),
            "response_format": "b64_json",
        }))
    }

    fn parse_image_response(&self, raw: Value) -> Result<Vec<ImageOutput>, AdapterError> {
        let data = raw["data"]
            .as_array()
            .ok_or_else(|| AdapterError::MissingField("data".to_string()))?;

        let mut outputs = Vec::new();
        for item in data {
            let b64 = item["b64_json"]
                .as_str()
                .ok_or_else(|| AdapterError::MissingField("b64_json".to_string()))?;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| AdapterError::ParseError(e.to_string()))?;
            let bytes_len = decoded.len() as u64;
            outputs.push(ImageOutput {
                file_path: String::new(),
                mime_type: "image/png".to_string(),
                size: None,
                bytes: bytes_len,
            });
        }
        Ok(outputs)
    }

    fn build_speech_request(&self, req: &AvixSpeechRequest) -> Result<Value, AdapterError> {
        Ok(json!({
            "model": req.model,
            "input": req.text,
            "voice": req.voice,
            "response_format": req.format.as_deref().unwrap_or("mp3"),
            "speed": req.speed.unwrap_or(1.0),
        }))
    }

    fn speech_endpoint(&self, req: &AvixSpeechRequest) -> Result<SpeechEndpoint, AdapterError> {
        Ok(SpeechEndpoint {
            url: "/v1/audio/speech".to_string(),
            headers: HashMap::new(),
            format: req.format.clone().unwrap_or_else(|| "mp3".to_string()),
        })
    }

    fn build_transcription_request(
        &self,
        req: &AvixTranscribeRequest,
        audio_bytes: &[u8],
    ) -> Result<MultipartRequest, AdapterError> {
        let mut fields = HashMap::new();
        fields.insert("model".to_string(), req.model.clone());
        if let Some(lang) = &req.language {
            fields.insert("language".to_string(), lang.clone());
        }
        if let Some(prompt) = &req.prompt {
            fields.insert("prompt".to_string(), prompt.clone());
        }

        Ok(MultipartRequest {
            url: "/v1/audio/transcriptions".to_string(),
            headers: HashMap::new(),
            fields,
            audio_bytes: audio_bytes.to_vec(),
            filename: "audio.wav".to_string(),
        })
    }

    fn parse_transcription_response(
        &self,
        raw: Value,
    ) -> Result<AvixTranscribeResponse, AdapterError> {
        let text = raw["text"]
            .as_str()
            .ok_or_else(|| AdapterError::MissingField("text".to_string()))?
            .to_string();

        let language = raw["language"].as_str().map(|s| s.to_string());
        let duration_sec = raw["duration"].as_f64().map(|d| d as f32);

        let segments = raw["segments"].as_array().map(|segs| {
            segs.iter()
                .map(|s| TranscribeSegment {
                    start: s["start"].as_f64().unwrap_or(0.0) as f32,
                    end: s["end"].as_f64().unwrap_or(0.0) as f32,
                    text: s["text"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        });

        Ok(AvixTranscribeResponse {
            text,
            language,
            duration_sec,
            segments,
        })
    }

    fn build_embed_request(&self, req: &AvixEmbedRequest) -> Result<Value, AdapterError> {
        let input = match &req.input {
            EmbedInput::Single(s) => json!([s]),
            EmbedInput::Batch(v) => json!(v),
        };
        Ok(json!({
            "model": req.model,
            "input": input,
        }))
    }

    fn parse_embed_response(&self, raw: Value) -> Result<AvixEmbedResponse, AdapterError> {
        let data = raw["data"]
            .as_array()
            .ok_or_else(|| AdapterError::MissingField("data".to_string()))?;

        let mut embeddings: Vec<Vec<f32>> = Vec::new();
        for item in data {
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| AdapterError::MissingField("embedding".to_string()))?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            embeddings.push(embedding);
        }

        let dimensions = embeddings.first().map(|e| e.len() as u32).unwrap_or(0);
        let input_tokens = raw["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;

        Ok(AvixEmbedResponse {
            embeddings,
            dimensions,
            usage: EmbedUsage { input_tokens },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_adapter() -> OpenAiAdapter {
        OpenAiAdapter::new()
    }

    fn make_metadata() -> super::super::CompleteMetadata {
        super::super::CompleteMetadata {
            agent_pid: 1,
            session_id: "sess-1".to_string(),
        }
    }

    #[test]
    fn test_translate_tools_function_format() {
        let adapter = make_adapter();
        let tools = vec![AvixToolDescriptor {
            name: "fs/read".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
        }];
        let result = adapter.translate_tools(&tools);
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "fs__read");
    }

    #[test]
    fn test_build_complete_request_has_model() {
        let adapter = make_adapter();
        let req = AvixCompleteRequest {
            provider: None,
            model: "gpt-4o".to_string(),
            messages: vec![json!({"role": "user", "content": "hello"})],
            system: None,
            max_tokens: Some(512),
            temperature: Some(0.7),
            stream: None,
            stop_sequences: None,
            tools: vec![],
            metadata: make_metadata(),
        };
        let body = adapter.build_complete_request(&req);
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["max_tokens"], 512);
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_parse_complete_response_text() {
        let adapter = make_adapter();
        let raw = json!({
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });
        let resp = adapter.parse_complete_response(raw).unwrap();
        assert_eq!(resp.provider, "openai");
        assert_eq!(resp.stop_reason, "stop");
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0]["text"], "Hello!");
    }

    #[test]
    fn test_parse_complete_response_tool_call() {
        let adapter = make_adapter();
        let raw = json!({
            "model": "gpt-4o",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "fs__read",
                            "arguments": "{\"path\": \"/tmp/file\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 10
            }
        });
        let resp = adapter.parse_complete_response(raw).unwrap();
        assert_eq!(resp.stop_reason, "tool_use");
        let tool_item = &resp.content[0];
        assert_eq!(tool_item["type"], "tool_use");
        assert_eq!(tool_item["name"], "fs/read");
    }

    #[test]
    fn test_parse_tool_call_unmangles() {
        let adapter = make_adapter();
        let raw = json!({
            "id": "call_xyz",
            "type": "function",
            "function": {
                "name": "fs__write",
                "arguments": "{\"path\": \"/tmp/file\", \"content\": \"data\"}"
            }
        });
        let call = adapter.parse_tool_call(&raw).unwrap();
        assert_eq!(call.call_id, "call_xyz");
        assert_eq!(call.name, "fs/write");
        assert_eq!(call.args["path"], "/tmp/file");
    }

    #[test]
    fn test_format_tool_result() {
        let adapter = make_adapter();
        let result = AvixToolResult {
            call_id: "call_abc".to_string(),
            output: json!({"status": "ok"}),
            error: None,
        };
        let formatted = adapter.format_tool_result(&result);
        assert_eq!(formatted["role"], "tool");
        assert_eq!(formatted["tool_call_id"], "call_abc");
    }

    #[test]
    fn test_build_image_request() {
        let adapter = make_adapter();
        let req = AvixImageRequest {
            provider: None,
            model: "dall-e-3".to_string(),
            prompt: "A cat".to_string(),
            negative_prompt: None,
            size: Some("1024x1024".to_string()),
            style: None,
            n: Some(1),
            metadata: make_metadata(),
        };
        let body = adapter.build_image_request(&req).unwrap();
        assert_eq!(body["model"], "dall-e-3");
        assert_eq!(body["response_format"], "b64_json");
        assert_eq!(body["n"], 1);
    }

    #[test]
    fn test_parse_image_response() {
        let adapter = make_adapter();
        // Encode some bytes in base64
        let fake_bytes = vec![0u8; 100];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&fake_bytes);
        let raw = json!({
            "data": [{"b64_json": b64}]
        });
        let outputs = adapter.parse_image_response(raw).unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].mime_type, "image/png");
        assert_eq!(outputs[0].bytes, 100);
    }

    #[test]
    fn test_build_speech_request() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
            provider: None,
            model: "tts-1".to_string(),
            text: "Hello world".to_string(),
            voice: "alloy".to_string(),
            format: Some("mp3".to_string()),
            speed: Some(1.0),
            stream: None,
            metadata: make_metadata(),
        };
        let body = adapter.build_speech_request(&req).unwrap();
        assert_eq!(body["model"], "tts-1");
        assert_eq!(body["voice"], "alloy");
        assert_eq!(body["response_format"], "mp3");
    }

    #[test]
    fn test_speech_endpoint() {
        let adapter = make_adapter();
        let req = AvixSpeechRequest {
            provider: None,
            model: "tts-1".to_string(),
            text: "Hello".to_string(),
            voice: "alloy".to_string(),
            format: Some("opus".to_string()),
            speed: None,
            stream: None,
            metadata: make_metadata(),
        };
        let endpoint = adapter.speech_endpoint(&req).unwrap();
        assert_eq!(endpoint.url, "/v1/audio/speech");
        assert_eq!(endpoint.format, "opus");
    }

    #[test]
    fn test_build_transcription_request() {
        let adapter = make_adapter();
        let req = AvixTranscribeRequest {
            provider: None,
            model: "whisper-1".to_string(),
            file_path: "/tmp/audio.wav".to_string(),
            language: Some("en".to_string()),
            prompt: None,
            granularity: None,
            metadata: make_metadata(),
        };
        let audio = b"fake audio data";
        let mreq = adapter.build_transcription_request(&req, audio).unwrap();
        assert_eq!(mreq.url, "/v1/audio/transcriptions");
        assert_eq!(mreq.fields["model"], "whisper-1");
        assert_eq!(mreq.fields["language"], "en");
        assert_eq!(mreq.audio_bytes, audio);
    }

    #[test]
    fn test_parse_transcription_response() {
        let adapter = make_adapter();
        let raw = json!({
            "text": "Hello, world!",
            "language": "en",
            "duration": 3.5,
            "segments": [
                {"start": 0.0, "end": 1.5, "text": "Hello,"},
                {"start": 1.5, "end": 3.5, "text": " world!"}
            ]
        });
        let resp = adapter.parse_transcription_response(raw).unwrap();
        assert_eq!(resp.text, "Hello, world!");
        assert_eq!(resp.language.as_deref(), Some("en"));
        assert!((resp.duration_sec.unwrap() - 3.5).abs() < 0.01);
        let segs = resp.segments.unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "Hello,");
    }

    #[test]
    fn test_build_embed_request_single() {
        let adapter = make_adapter();
        let req = AvixEmbedRequest {
            provider: None,
            model: "text-embedding-3-small".to_string(),
            input: EmbedInput::Single("Hello".to_string()),
            metadata: make_metadata(),
        };
        let body = adapter.build_embed_request(&req).unwrap();
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"][0], "Hello");
    }

    #[test]
    fn test_build_embed_request_batch() {
        let adapter = make_adapter();
        let req = AvixEmbedRequest {
            provider: None,
            model: "text-embedding-3-small".to_string(),
            input: EmbedInput::Batch(vec!["Hello".to_string(), "World".to_string()]),
            metadata: make_metadata(),
        };
        let body = adapter.build_embed_request(&req).unwrap();
        assert_eq!(body["input"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_parse_embed_response() {
        let adapter = make_adapter();
        let raw = json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0}
            ],
            "usage": {"prompt_tokens": 5, "total_tokens": 5}
        });
        let resp = adapter.parse_embed_response(raw).unwrap();
        assert_eq!(resp.embeddings.len(), 1);
        assert_eq!(resp.dimensions, 3);
        assert!((resp.embeddings[0][0] - 0.1).abs() < 0.001);
        assert_eq!(resp.usage.input_tokens, 5);
    }
}
