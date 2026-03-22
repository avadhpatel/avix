use avix_core::llm_svc::adapter::{AvixToolCall, AvixToolResult};
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

// ---- RoutingEngine tests ----

#[tokio::test]
async fn routing_resolves_default_text_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let provider = engine.resolve(Modality::Text, None).await.unwrap();
    assert_eq!(provider.name, "anthropic");
}

#[tokio::test]
async fn routing_resolves_explicit_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let provider = engine
        .resolve(Modality::Image, Some("openai"))
        .await
        .unwrap();
    assert_eq!(provider.name, "openai");
}

#[tokio::test]
async fn routing_rejects_modality_mismatch() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    // anthropic doesn't support image
    let err = engine
        .resolve(Modality::Image, Some("anthropic"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not support"));
}

#[tokio::test]
async fn routing_rejects_unknown_provider() {
    let config = make_llm_config();
    let engine = RoutingEngine::from_config(&config);
    let err = engine
        .resolve(Modality::Text, Some("unknown-provider"))
        .await
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

#[test]
fn avix_tool_result_fields() {
    let r = AvixToolResult {
        call_id: "id-2".into(),
        output: json!({"status": "ok"}),
        error: None,
    };
    assert_eq!(r.call_id, "id-2");
    assert!(r.error.is_none());

    let r_err = AvixToolResult {
        call_id: "id-3".into(),
        output: json!(null),
        error: Some("EPERM".into()),
    };
    assert_eq!(r_err.error.as_deref(), Some("EPERM"));
}
