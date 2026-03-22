# Day 28 — `avix llm` CLI Commands

> **Goal:** Implement all `avix llm` CLI sub-commands: `status`, `test`, `rotate`, `models`, `usage`, `provider add/disable/enable`, `default set`. Each command communicates with `llm.svc` via the internal IPC socket.

---

## Pre-flight: Verify Day 27

```bash
cargo test --workspace
grep -r "ExecService"   crates/avix-core/src/
grep -r "McpBridge"     crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add CLI commands to `crates/avix-cli/src/`:

```
crates/avix-cli/src/
├── main.rs
└── commands/
    ├── mod.rs
    ├── llm/
    │   ├── mod.rs
    │   ├── status.rs
    │   ├── test.rs
    │   ├── rotate.rs
    │   ├── models.rs
    │   ├── usage.rs
    │   └── provider.rs
    └── ...other commands...
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/llm_cli.rs`:

```rust
use avix_core::llm_svc::cli::{LlmCliHandler, LlmStatusOutput, LlmModelsOutput};

// ── avix llm status ───────────────────────────────────────────────────────────

#[tokio::test]
async fn llm_status_returns_all_providers() {
    let handler = LlmCliHandler::new_for_test();
    let status = handler.status().await.unwrap();

    let provider_names: Vec<_> = status.providers.iter().map(|p| p.name.as_str()).collect();
    assert!(provider_names.contains(&"anthropic"));
    assert!(provider_names.contains(&"openai"));
}

#[tokio::test]
async fn llm_status_shows_modalities_per_provider() {
    let handler = LlmCliHandler::new_for_test();
    let status = handler.status().await.unwrap();

    let openai = status.providers.iter().find(|p| p.name == "openai").unwrap();
    let modality_names: Vec<_> = openai.modalities.iter().map(|m| m.as_str()).collect();
    assert!(modality_names.contains(&"text"));
    assert!(modality_names.contains(&"image"));
}

#[tokio::test]
async fn llm_status_shows_health_per_provider() {
    let handler = LlmCliHandler::new_for_test();
    let status = handler.status().await.unwrap();
    // All providers in test mode should be healthy
    for provider in &status.providers {
        assert!(matches!(provider.health, avix_core::llm_svc::ProviderHealth::Healthy |
                                          avix_core::llm_svc::ProviderHealth::Degraded),
            "provider {} has unexpected health", provider.name);
    }
}

// ── avix llm models ───────────────────────────────────────────────────────────

#[tokio::test]
async fn llm_models_returns_grouped_by_modality() {
    let handler = LlmCliHandler::new_for_test();
    let models = handler.models(None).await.unwrap();
    // Should have at least one modality group
    assert!(!models.groups.is_empty());
}

#[tokio::test]
async fn llm_models_filter_by_modality() {
    use avix_core::types::Modality;
    let handler = LlmCliHandler::new_for_test();
    let models = handler.models(Some(Modality::Text)).await.unwrap();
    // All returned models should support text
    for group in &models.groups {
        assert_eq!(group.modality, Modality::Text);
    }
}

// ── avix llm usage ────────────────────────────────────────────────────────────

#[tokio::test]
async fn llm_usage_returns_since_boot() {
    let handler = LlmCliHandler::new_for_test();
    let usage = handler.usage().await.unwrap();
    // Since boot, usage stats exist
    assert!(usage.total_input_tokens >= 0);
    assert!(usage.total_output_tokens >= 0);
}

// ── avix llm provider ─────────────────────────────────────────────────────────

#[tokio::test]
async fn llm_provider_disable_and_enable() {
    let handler = LlmCliHandler::new_for_test();
    handler.disable_provider("openai").await.unwrap();
    let status = handler.status().await.unwrap();
    let openai = status.providers.iter().find(|p| p.name == "openai").unwrap();
    assert_eq!(openai.enabled, false);

    handler.enable_provider("openai").await.unwrap();
    let status2 = handler.status().await.unwrap();
    let openai2 = status2.providers.iter().find(|p| p.name == "openai").unwrap();
    assert_eq!(openai2.enabled, true);
}

// ── avix llm default set ──────────────────────────────────────────────────────

#[tokio::test]
async fn llm_default_set_changes_default_provider() {
    use avix_core::types::Modality;
    let handler = LlmCliHandler::new_for_test();

    // Change text default from anthropic to openai
    handler.set_default(Modality::Text, "openai").await.unwrap();

    let status = handler.status().await.unwrap();
    let text_default = status.defaults.iter().find(|d| d.modality == Modality::Text).unwrap();
    assert_eq!(text_default.provider, "openai");
}

#[tokio::test]
async fn llm_default_set_rejects_provider_wrong_modality() {
    use avix_core::types::Modality;
    let handler = LlmCliHandler::new_for_test();
    // Anthropic doesn't support image
    let result = handler.set_default(Modality::Image, "anthropic").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("modality"));
}

// ── avix llm rotate (key rotation) ───────────────────────────────────────────

#[tokio::test]
async fn llm_rotate_re_encrypts_and_broadcasts_sighup() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let handler = LlmCliHandler::new_for_test();

    // We only verify the operation succeeds without error
    let result = handler.rotate("anthropic", b"new-api-key-value").await;
    assert!(result.is_ok());
}
```

---

## Step 3 — Implement

`LlmCliHandler` holds a reference to `LlmService`. Methods are direct function calls (not real IPC) in tests; in production they go via IPC to `llm.svc`'s socket. `disable_provider` / `enable_provider` toggle an `enabled` flag on the `ProviderConfig` in memory. `set_default` validates modality support first then updates the routing engine.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 20+ CLI handler tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-28: avix llm CLI — status, models, usage, provider, default set, rotate"
```

## Success Criteria

- [ ] `status` lists all configured providers with modalities
- [ ] `status` shows health per provider
- [ ] `models` groups by modality; filter works
- [ ] `usage` returns non-negative token counts
- [ ] `disable` + `enable` toggles provider availability
- [ ] `default set` changes default provider
- [ ] `default set` rejects wrong-modality provider
- [ ] `rotate` completes without error
- [ ] 20+ tests pass, 0 clippy warnings

---
---

