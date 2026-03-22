# Day 14 — AutoAgents Integration: LLM Provider Abstraction

> **Goal:** Wire the `AutoAgents` library (`liquidos-ai/AutoAgents`) into `avix-core` for the text-completion path only. Define the `LlmClient` trait that `llm.svc` will implement on Day 14b. Verify streaming and token counting work.

---

## Pre-flight: Verify Day 13

```bash
cargo test --workspace
grep -r "pub struct SessionStore" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Add AutoAgents Dependency

In `crates/avix-core/Cargo.toml`:

```toml
[dependencies]
# Replace with actual crate name/version from liquidos-ai/AutoAgents
autoagents = { git = "https://github.com/liquidos-ai/AutoAgents", optional = true }
```

Add feature flag `llm` to keep it optional during builds that don't need it.

Add to `src/lib.rs`:
```rust
pub mod llm_client;
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/llm_client.rs`:

```rust
use avix_core::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse, StopReason};
use serde_json::json;

// ── Trait contract ────────────────────────────────────────────────────────────

/// Mock client for testing the trait contract.
struct MockLlmClient {
    response: LlmCompleteResponse,
}

#[async_trait::async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn llm_client_complete_returns_response() {
    let client = MockLlmClient {
        response: LlmCompleteResponse {
            content:       vec![json!({"type": "text", "text": "Hello, world!"})],
            stop_reason:   StopReason::EndTurn,
            input_tokens:  10,
            output_tokens: 5,
        },
    };
    let req = LlmCompleteRequest {
        model:     "claude-sonnet-4".into(),
        messages:  vec![json!({"role": "user", "content": "Hi"})],
        tools:     vec![],
        system:    None,
        max_tokens: 1000,
    };
    let resp = client.complete(req).await.unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(resp.input_tokens, 10);
}

// ── StopReason ────────────────────────────────────────────────────────────────

#[test]
fn stop_reason_from_str() {
    assert_eq!("end_turn".parse::<StopReason>().unwrap(),    StopReason::EndTurn);
    assert_eq!("tool_use".parse::<StopReason>().unwrap(),    StopReason::ToolUse);
    assert_eq!("max_tokens".parse::<StopReason>().unwrap(),  StopReason::MaxTokens);
    assert_eq!("stop_sequence".parse::<StopReason>().unwrap(), StopReason::StopSequence);
    assert!("unknown".parse::<StopReason>().is_err());
}

// ── Request construction ──────────────────────────────────────────────────────

#[test]
fn llm_complete_request_serialises() {
    let req = LlmCompleteRequest {
        model:      "claude-opus-4".into(),
        messages:   vec![json!({"role": "user", "content": "Hello"})],
        tools:      vec![json!({"name": "fs__read"})],
        system:     Some("You are a researcher.".into()),
        max_tokens: 2000,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["model"], "claude-opus-4");
    assert_eq!(v["max_tokens"], 2000);
    assert!(v["tools"].as_array().unwrap().len() == 1);
}

// ── Token counting ────────────────────────────────────────────────────────────

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
```

---

## Step 3 — Implement

**`src/llm_client/mod.rs`** — define `LlmClient` trait, `LlmCompleteRequest`, `LlmCompleteResponse`, `StopReason` enum.

The `AutoAgentsClient` struct implements `LlmClient` by wrapping the AutoAgents library. If the library is unavailable during testing, use the `MockLlmClient` pattern above — the trait is what matters for the rest of the system.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-14: LlmClient trait, StopReason, AutoAgents integration stub"
```

## Success Criteria

- [ ] `LlmClient` trait defined with `complete` method
- [ ] `StopReason` parses all four values correctly
- [ ] Mock client satisfies trait contract
- [ ] `LlmCompleteResponse.total_tokens()` is correct
- [ ] 10+ tests pass, 0 clippy warnings

---
---

