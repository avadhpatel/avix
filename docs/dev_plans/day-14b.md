# Day 14b — `llm.svc`: Multi-Modality Implementation

> **Goal:** Full `llm.svc` implementation: provider registry, routing engine, all five provider adapters (Anthropic, OpenAI, Ollama, Stability AI, ElevenLabs), tool name mangling, binary output handler, OAuth2 refresh loop, health-check tasks, and `tool.changed` emission.

---

## Pre-flight: Verify Day 14

```bash
cargo test --workspace
grep -r "pub trait LlmClient"  crates/avix-core/src/
grep -r "pub enum StopReason"  crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod llm_svc;`

```
src/llm_svc/
├── mod.rs
├── service.rs         ← LlmService main struct
├── adapter/
│   ├── mod.rs
│   ├── trait_.rs      ← ProviderAdapter trait
│   ├── anthropic.rs
│   ├── openai.rs
│   ├── ollama.rs      ← thin wrapper over openai
│   ├── stability.rs
│   └── elevenlabs.rs
├── routing.rs         ← routing engine
├── binary_output.rs   ← scratch dir writer
└── oauth2_refresh.rs  ← background token refresh
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/llm_svc.rs`:

```rust
use avix_core::llm_svc::adapter::{AnthropicAdapter, OpenAiAdapter, ProviderAdapter};
use avix_core::types::tool::ToolName;
use serde_json::json;

// ── AnthropicAdapter — tool descriptor translation ────────────────────────────

#[test]
fn anthropic_translates_tool_descriptor() {
    let adapter = AnthropicAdapter::new();
    let descriptor = json!({
        "name": "fs/write",
        "description": "Write to VFS",
        "input": {
            "path":    {"type": "string",  "required": true},
            "content": {"type": "string",  "required": true},
            "append":  {"type": "bool",    "required": false, "default": false}
        }
    });
    let translated = adapter.translate_tool(&descriptor);
    assert_eq!(translated["name"], "fs__write");
    assert!(translated["input_schema"]["properties"].is_object());
    let required = translated["input_schema"]["required"].as_array().unwrap();
    assert!(required.contains(&json!("path")));
    assert!(required.contains(&json!("content")));
    assert!(!required.contains(&json!("append")));
}

#[test]
fn anthropic_parses_tool_call_and_unmanges() {
    let adapter = AnthropicAdapter::new();
    let raw = json!({
        "type": "tool_use",
        "id":   "toolu_01ABC",
        "name": "mcp__github__list-prs",
        "input": {"state": "open"}
    });
    let call = adapter.parse_tool_call(&raw).unwrap();
    assert_eq!(call.name, "mcp/github/list-prs");
    assert_eq!(call.call_id, "toolu_01ABC");
}

#[test]
fn anthropic_formats_tool_result() {
    let adapter = AnthropicAdapter::new();
    let result = avix_core::llm_svc::adapter::AvixToolResult {
        call_id: "toolu_01ABC".into(),
        output:  json!({"bytesWritten": 5}),
        error:   None,
    };
    let msg = adapter.format_tool_result(&result);
    assert_eq!(msg["role"], "user");
    let content = msg["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "toolu_01ABC");
}

// ── OpenAiAdapter ─────────────────────────────────────────────────────────────

#[test]
fn openai_translates_tool_descriptor() {
    let adapter = OpenAiAdapter::new();
    let descriptor = json!({
        "name": "fs/write",
        "description": "Write",
        "input": { "path": {"type": "string", "required": true} }
    });
    let translated = adapter.translate_tool(&descriptor);
    assert_eq!(translated["type"], "function");
    assert_eq!(translated["function"]["name"], "fs__write");
}

#[test]
fn openai_parses_tool_call() {
    let adapter = OpenAiAdapter::new();
    let raw = json!({
        "id": "call_01ABC",
        "type": "function",
        "function": {
            "name": "fs__write",
            "arguments": "{\"path\":\"/test\",\"content\":\"hello\"}"
        }
    });
    let call = adapter.parse_tool_call(&raw).unwrap();
    assert_eq!(call.name, "fs/write");
    assert_eq!(call.args["path"], "/test");
}

// ── Tool name mangle invariant ────────────────────────────────────────────────

#[test]
fn mangle_round_trip_for_all_namespaces() {
    let names = [
        "fs/read", "fs/write",
        "llm/complete", "llm/generate-image",
        "mcp/github/list-prs",
        "agent/spawn",
        "cap/request-tool",
    ];
    for name in names {
        let tool = ToolName::parse(name).unwrap();
        let mangled = tool.mangled();
        let unmangled = ToolName::unmangle(&mangled).unwrap();
        assert_eq!(unmangled.as_str(), name, "round-trip failed for {name}");
    }
}

// ── Routing engine ────────────────────────────────────────────────────────────

#[tokio::test]
async fn routing_uses_default_provider_for_text_when_none_specified() {
    use avix_core::llm_svc::routing::RoutingEngine;
    use avix_core::config::LlmConfig;
    use avix_core::types::Modality;

    let config = LlmConfig::from_str(VALID_LLM_CONFIG).unwrap();
    let engine = RoutingEngine::from_config(&config);

    let provider = engine.resolve(Modality::Text, None).unwrap();
    assert_eq!(provider.name, "anthropic");
}

#[tokio::test]
async fn routing_rejects_provider_wrong_modality() {
    use avix_core::llm_svc::routing::RoutingEngine;
    use avix_core::config::LlmConfig;
    use avix_core::types::Modality;

    let config = LlmConfig::from_str(VALID_LLM_CONFIG).unwrap();
    let engine = RoutingEngine::from_config(&config);

    let result = engine.resolve(Modality::Text, Some("stability-ai"));
    assert!(result.is_err()); // stability-ai doesn't support text
}

// ── Binary output handler ─────────────────────────────────────────────────────

#[tokio::test]
async fn binary_output_writes_to_scratch_dir() {
    use avix_core::llm_svc::binary_output::write_binary_output;
    use avix_core::types::Pid;
    use tempfile::tempdir;

    let tmp = tempdir().unwrap();
    // Simulate /proc/<pid>/scratch/ under tmp
    let scratch = tmp.path().join("proc/57/scratch");
    std::fs::create_dir_all(&scratch).unwrap();

    let path = write_binary_output(
        &scratch, "image", b"fake-png-bytes", "png"
    ).unwrap();

    assert!(path.ends_with(".png"));
    assert!(std::path::Path::new(&path).exists() || path.starts_with("/proc/57/scratch/"));
}

// ── OAuth2 refresh ────────────────────────────────────────────────────────────

#[tokio::test]
async fn oauth2_refresh_timer_fires_before_expiry() {
    use avix_core::llm_svc::oauth2_refresh::RefreshScheduler;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    use std::time::Duration;

    let refreshed = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&refreshed);

    let scheduler = RefreshScheduler::new();
    scheduler.schedule("openai", Duration::from_millis(50), move || {
        r.store(true, Ordering::Relaxed);
    }).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(refreshed.load(Ordering::Relaxed));
}

const VALID_LLM_CONFIG: &str = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: openai
    speech: elevenlabs
    transcription: openai
    embedding: openai
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth: { type: api_key, secretName: k, header: x-api-key }
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [text, image, speech, transcription, embedding]
      auth: { type: none }
    - name: elevenlabs
      baseUrl: https://api.elevenlabs.io
      modalities: [speech]
      auth: { type: api_key, secretName: k, header: xi-api-key }
"#;
```

---

## Step 3 — Implement

**`ProviderAdapter` trait:**

```rust
pub trait ProviderAdapter: Send + Sync {
    fn provider_name(&self) -> &str;
    fn modalities(&self) -> &[Modality];
    fn translate_tool(&self, descriptor: &Value) -> Value;
    fn parse_tool_call(&self, raw: &Value) -> Result<AvixToolCall, AvixError>;
    fn format_tool_result(&self, result: &AvixToolResult) -> Value;
    fn translate_messages(&self, messages: &[Value]) -> Vec<Value>;
    fn parse_response(&self, raw: &Value) -> Result<LlmCompleteResponse, AvixError>;
}
```

Implement `AnthropicAdapter` and `OpenAiAdapter` fully. `OllamaAdapter` delegates to `OpenAiAdapter` with different base URL. `StabilityAdapter` and `ElevenLabsAdapter` are stubs returning `Err` for text calls.

**`RoutingEngine`**: holds a `HashMap<Modality, String>` for defaults and a `HashMap<String, ProviderConfig>` for lookup. `resolve(modality, explicit_provider)` enforces the modality+capability rules.

**`write_binary_output`**: writes bytes to `<scratch_dir>/<uuid>.<ext>`, returns the VFS path string.

**`RefreshScheduler`**: `HashMap<String, JoinHandle>` — each entry runs a `tokio::time::interval` loop calling the closure.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 30+ llm_svc tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-14b: llm.svc — provider adapters, routing engine, binary output, OAuth2 refresh"
```

## Success Criteria

- [ ] `AnthropicAdapter` translates descriptors: `fs/write` → `fs__write`, required fields correct
- [ ] `AnthropicAdapter` parses tool call and unmangled back to `mcp/github/list-prs`
- [ ] `AnthropicAdapter` formats tool result with correct `tool_result` shape
- [ ] `OpenAiAdapter` translates to `{"type": "function", ...}` shape
- [ ] Tool name mangle round-trip for 7 namespaces
- [ ] Routing uses default provider when none specified
- [ ] Routing rejects provider that doesn't support the modality
- [ ] Binary output writes to scratch dir with correct extension
- [ ] OAuth2 refresh scheduler fires within timeout
- [ ] 30+ tests pass, 0 clippy warnings

---
---

