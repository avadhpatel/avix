use avix_core::llm_svc::adapter::{
    AnthropicAdapter, AvixToolCall, AvixToolResult, OpenAiAdapter, ProviderAdapter,
};
use avix_core::llm_svc::binary_output::write_binary_output;
use avix_core::llm_svc::routing::RoutingEngine;
use avix_core::types::Modality;
use serde_json::json;
use tempfile::tempdir;

fn make_llm_config() -> avix_core::config::LlmConfig {
    avix_core::config::LlmConfig::from_str(
        r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: ANTHROPIC_API_KEY
        header: x-api-key
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [image, speech, transcription, embedding]
      auth:
        type: api_key
        secretName: OPENAI_API_KEY
        header: Authorization
"#,
    )
    .unwrap()
}

// ---- Anthropic adapter tests ----

#[test]
fn anthropic_translate_tool_mangles_name() {
    let adapter = AnthropicAdapter::new();
    let descriptor = json!({
        "name": "fs/read",
        "description": "Read a file",
        "input": {
            "path": {"type": "string", "required": true}
        }
    });
    let translated = adapter.translate_tool(&descriptor);
    assert_eq!(translated["name"], "fs__read");
    assert!(translated["input_schema"]["properties"]["path"].is_object());
    assert_eq!(translated["input_schema"]["required"][0], "path");
}

#[test]
fn anthropic_translate_tool_optional_field() {
    let adapter = AnthropicAdapter::new();
    let descriptor = json!({
        "name": "fs/list",
        "description": "List files",
        "input": {
            "path": {"type": "string", "required": true},
            "recursive": {"type": "bool", "required": false}
        }
    });
    let translated = adapter.translate_tool(&descriptor);
    let req: Vec<&str> = translated["input_schema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(req.contains(&"path"));
    assert!(!req.contains(&"recursive"));
    assert_eq!(
        translated["input_schema"]["properties"]["recursive"]["type"],
        "boolean"
    );
}

#[test]
fn anthropic_parse_tool_call_unmangles_name() {
    let adapter = AnthropicAdapter::new();
    let raw = json!({
        "id": "call-1",
        "name": "fs__read",
        "input": {"path": "/etc/passwd"}
    });
    let call = adapter.parse_tool_call(&raw).unwrap();
    assert_eq!(call.call_id, "call-1");
    assert_eq!(call.name, "fs/read");
    assert_eq!(call.args["path"], "/etc/passwd");
}

#[test]
fn anthropic_format_tool_result_success() {
    let adapter = AnthropicAdapter::new();
    let result = AvixToolResult {
        call_id: "call-2".into(),
        output: json!("file contents here"),
        error: None,
    };
    let formatted = adapter.format_tool_result(&result);
    assert_eq!(formatted["role"], "user");
    let content = &formatted["content"][0];
    assert_eq!(content["type"], "tool_result");
    assert_eq!(content["tool_use_id"], "call-2");
    assert!(content.get("is_error").is_none() || content["is_error"].is_null());
}

#[test]
fn anthropic_format_tool_result_error() {
    let adapter = AnthropicAdapter::new();
    let result = AvixToolResult {
        call_id: "call-3".into(),
        output: json!(null),
        error: Some("EPERM: permission denied".into()),
    };
    let formatted = adapter.format_tool_result(&result);
    let content = &formatted["content"][0];
    assert_eq!(content["is_error"], true);
    assert_eq!(content["content"], "EPERM: permission denied");
}

// ---- OpenAI adapter tests ----

#[test]
fn openai_translate_tool_wraps_in_function() {
    let adapter = OpenAiAdapter::new();
    let descriptor = json!({
        "name": "fs/write",
        "description": "Write a file",
        "input": {
            "path": {"type": "string", "required": true},
            "content": {"type": "string", "required": true}
        }
    });
    let translated = adapter.translate_tool(&descriptor);
    assert_eq!(translated["type"], "function");
    assert_eq!(translated["function"]["name"], "fs__write");
    assert!(translated["function"]["parameters"]["properties"]["path"].is_object());
}

#[test]
fn openai_parse_tool_call_unmangles() {
    let adapter = OpenAiAdapter::new();
    let raw = json!({
        "id": "call-oai-1",
        "function": {
            "name": "llm__complete",
            "arguments": "{\"model\": \"gpt-4\"}"
        }
    });
    let call = adapter.parse_tool_call(&raw).unwrap();
    assert_eq!(call.name, "llm/complete");
    assert_eq!(call.args["model"], "gpt-4");
}

#[test]
fn openai_format_tool_result() {
    let adapter = OpenAiAdapter::new();
    let result = AvixToolResult {
        call_id: "call-oai-2".into(),
        output: json!({"status": "ok"}),
        error: None,
    };
    let formatted = adapter.format_tool_result(&result);
    assert_eq!(formatted["role"], "tool");
    assert_eq!(formatted["tool_call_id"], "call-oai-2");
}

// ---- RoutingEngine tests ----

#[test]
fn routing_resolves_default_text_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let provider = engine.resolve(Modality::Text, None).unwrap();
    assert_eq!(provider.name, "anthropic");
}

#[test]
fn routing_resolves_explicit_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let provider = engine.resolve(Modality::Image, Some("openai")).unwrap();
    assert_eq!(provider.name, "openai");
}

#[test]
fn routing_rejects_modality_mismatch() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    // anthropic doesn't support image
    let err = engine
        .resolve(Modality::Image, Some("anthropic"))
        .unwrap_err();
    assert!(err.to_string().contains("does not support"));
}

#[test]
fn routing_rejects_unknown_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let err = engine
        .resolve(Modality::Text, Some("unknown-provider"))
        .unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// ---- binary_output tests ----

#[test]
fn binary_output_writes_file_with_correct_ext() {
    let tmp = tempdir().unwrap();
    let data = b"fake png bytes";
    let path = write_binary_output(tmp.path(), "image", data, "png").unwrap();
    assert!(path.ends_with(".png"));
    assert!(std::path::Path::new(&path).exists());
    let read_back = std::fs::read(&path).unwrap();
    assert_eq!(read_back, data);
}

#[test]
fn binary_output_unique_filenames() {
    let tmp = tempdir().unwrap();
    let p1 = write_binary_output(tmp.path(), "audio", b"data1", "mp3").unwrap();
    let p2 = write_binary_output(tmp.path(), "audio", b"data2", "mp3").unwrap();
    assert_ne!(p1, p2);
}

// ---- tool call struct tests ----

#[test]
fn avix_tool_call_serialises() {
    let call = AvixToolCall {
        call_id: "id-1".into(),
        name: "fs/read".into(),
        args: json!({"path": "/tmp/file"}),
    };
    let v = serde_json::to_value(&call).unwrap();
    assert_eq!(v["call_id"], "id-1");
    assert_eq!(v["name"], "fs/read");
}
