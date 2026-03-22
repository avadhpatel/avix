# Day 16 — RuntimeExecutor: Full 7-Step Turn Loop + Tool Call Validation

> **Goal:** Implement the complete 7-step turn loop: tool list refresh on `tool.changed`, system prompt construction, `llm/complete` dispatch, stop-reason interpretation, capability/budget/HIL validation, Category 1 and Category 2 tool dispatch, and result injection back into messages.

---

## Pre-flight: Verify Day 15

```bash
cargo test --workspace
grep -r "pub struct RuntimeExecutor" crates/avix-core/src/
grep -r "fn build_system_prompt"     crates/avix-core/src/
grep -r "fn inject_pending_message"  crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Extend Executor Module

Add to `src/executor/`:

```
src/executor/
├── ...existing...
├── turn_loop.rs       ← 7-step loop
├── validation.rs      ← capability + budget checks
└── stop_reason.rs     ← TurnAction enum and interpretation
```

---

## Step 2 — Write Tests First

Add to `crates/avix-core/tests/runtime_executor.rs`:

```rust
use avix_core::executor::stop_reason::{TurnAction, interpret_stop_reason};
use avix_core::llm_client::{LlmCompleteResponse, StopReason};

// ── Stop-reason interpretation ────────────────────────────────────────────────

#[test]
fn end_turn_returns_result_action() {
    let resp = LlmCompleteResponse {
        content:       vec![serde_json::json!({"type": "text", "text": "done"})],
        stop_reason:   StopReason::EndTurn,
        input_tokens:  10,
        output_tokens: 5,
    };
    let action = interpret_stop_reason(&resp);
    assert!(matches!(action, TurnAction::ReturnResult(_)));
}

#[test]
fn tool_use_returns_dispatch_action() {
    let resp = LlmCompleteResponse {
        content: vec![serde_json::json!({
            "type": "tool_use",
            "id": "t1",
            "name": "fs__read",
            "input": {"path": "/test"}
        })],
        stop_reason:   StopReason::ToolUse,
        input_tokens:  10,
        output_tokens: 5,
    };
    let action = interpret_stop_reason(&resp);
    assert!(matches!(action, TurnAction::DispatchTools(_)));
    if let TurnAction::DispatchTools(calls) = action {
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "fs/read"); // unmangled
    }
}

#[test]
fn max_tokens_returns_summarise_action() {
    let resp = LlmCompleteResponse {
        content: vec![], stop_reason: StopReason::MaxTokens,
        input_tokens: 200000, output_tokens: 0,
    };
    assert!(matches!(interpret_stop_reason(&resp), TurnAction::SummariseContext));
}

#[test]
fn stop_sequence_treated_as_end_turn() {
    let resp = LlmCompleteResponse {
        content: vec![serde_json::json!({"type": "text", "text": "DONE"})],
        stop_reason: StopReason::StopSequence,
        input_tokens: 5, output_tokens: 2,
    };
    assert!(matches!(interpret_stop_reason(&resp), TurnAction::ReturnResult(_)));
}

// ── Capability validation ─────────────────────────────────────────────────────

#[tokio::test]
async fn tool_call_rejected_when_not_in_token() {
    use avix_core::executor::validation::validate_tool_call;

    let token = token_with_caps(&["fs:read"]); // no send_email
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "t1".into(), name: "send_email".into(), args: serde_json::json!({})
    };
    let result = validate_tool_call(&token, &call, &Default::default());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not granted") ||
            result.unwrap_err().to_string().contains("Tool not granted"));
}

// ── Budget enforcement ────────────────────────────────────────────────────────

#[tokio::test]
async fn tool_call_rejected_when_budget_zero() {
    use avix_core::executor::validation::{validate_tool_call, ToolBudgets};
    use std::collections::HashMap;

    let token = token_with_caps(&["send_email"]);
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "t1".into(), name: "send_email".into(), args: serde_json::json!({})
    };
    let mut budgets = ToolBudgets::default();
    budgets.set("send_email", 0); // exhausted

    let result = validate_tool_call(&token, &call, &budgets);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("budget"));
}

#[tokio::test]
async fn tool_call_allowed_when_budget_positive() {
    use avix_core::executor::validation::{validate_tool_call, ToolBudgets};

    let token = token_with_caps(&["send_email"]);
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "t1".into(), name: "send_email".into(), args: serde_json::json!({})
    };
    let mut budgets = ToolBudgets::default();
    budgets.set("send_email", 3);

    assert!(validate_tool_call(&token, &call, &budgets).is_ok());
}

// ── Tool list refresh on tool.changed ─────────────────────────────────────────

#[tokio::test]
async fn tool_list_excludes_removed_tool_after_changed_event() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["llm:inference"]),
        },
        registry.clone(),
    ).await.unwrap();

    // Simulate tool.changed: remove "llm/complete"
    executor.handle_tool_changed("removed", "llm/complete", "Provider down").await;

    let tool_list = executor.current_tool_list();
    let names: Vec<_> = tool_list.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"llm__complete"));
}

// ── Category 2 dispatch ───────────────────────────────────────────────────────

#[tokio::test]
async fn agent_spawn_translates_to_kernel_proc_spawn() {
    use avix_core::executor::MockKernelHandle;

    let mock_kernel = std::sync::Arc::new(MockKernelHandle::new());
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry_and_kernel(
        SpawnParams {
            pid: Pid::new(57), agent_name: "orchestrator".into(),
            goal: "g".into(), spawned_by: "alice".into(),
            token: token_with_caps(&["spawn"]),
        },
        registry, mock_kernel.clone(),
    ).await.unwrap();

    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "c1".into(),
        name: "agent/spawn".into(),
        args: serde_json::json!({
            "agent": "researcher",
            "goal": "Find Q3 revenue",
            "capabilities": ["web"],
            "waitForResult": false
        }),
    };

    executor.dispatch_category2(&call).await.unwrap();
    assert!(mock_kernel.received_proc_spawn("researcher").await);
}

#[tokio::test]
async fn cap_request_tool_triggers_resource_request() {
    use avix_core::executor::MockKernelHandle;

    let mock_kernel = std::sync::Arc::new(MockKernelHandle::new());
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry_and_kernel(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(), token: token_with_caps(&[]),
        },
        registry, mock_kernel.clone(),
    ).await.unwrap();

    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "c2".into(),
        name: "cap/request-tool".into(),
        args: serde_json::json!({"tool": "send_email", "reason": "Need to notify"}),
    };

    // Mock kernel immediately approves
    mock_kernel.auto_approve_resource_request().await;
    let result = executor.dispatch_category2(&call).await;
    assert!(result.is_ok());
}
```

---

## Step 3 — Implement

**`src/executor/stop_reason.rs`**:

```rust
pub enum TurnAction {
    ReturnResult(String),
    DispatchTools(Vec<AvixToolCall>),
    SummariseContext,
}

pub fn interpret_stop_reason(resp: &LlmCompleteResponse) -> TurnAction {
    match resp.stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => {
            let text = resp.content.iter()
                .filter_map(|c| c["text"].as_str())
                .collect::<Vec<_>>().join("");
            TurnAction::ReturnResult(text)
        }
        StopReason::ToolUse => {
            let calls = resp.content.iter()
                .filter(|c| c["type"] == "tool_use")
                .map(|c| AvixToolCall {
                    call_id: c["id"].as_str().unwrap_or("").to_string(),
                    name: ToolName::unmangle(c["name"].as_str().unwrap_or(""))
                            .map(|n| n.as_str().to_string())
                            .unwrap_or_default(),
                    args: c["input"].clone(),
                })
                .collect();
            TurnAction::DispatchTools(calls)
        }
        StopReason::MaxTokens => TurnAction::SummariseContext,
    }
}
```

**`src/executor/validation.rs`** — `validate_tool_call` checks `token.has_tool(name)` and `budgets.remaining(name) > 0`.

**`src/executor/turn_loop.rs`** — `run_one_turn` implements steps 1–7.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 25+ new executor tests
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-16: turn loop — stop-reason handling, cap/budget validation, Cat2 dispatch"
```

## Success Criteria

- [ ] All 4 `stopReason` values produce correct `TurnAction`
- [ ] Tool call rejected if not in capability token
- [ ] Tool call rejected when budget is 0
- [ ] `tool.changed` removes tool from next call's tool list
- [ ] `agent/spawn` dispatches `kernel/proc/spawn`
- [ ] `cap/request-tool` triggers resource request
- [ ] 25+ tests pass, 0 clippy warnings

---
---

