# Day 18 — RuntimeExecutor: Main Execution Loop Integration

> **Goal:** Wire everything together into the full turn loop: tool list refresh, prompt construction, llm/complete call, stop-reason dispatch, tool result injection, token renewal transparency, and `maxToolChainLength` enforcement.

---

## Pre-flight: Verify Day 17

```bash
cargo test --workspace
grep -r "HilApprover"       crates/avix-core/src/
grep -r "ApprovalTokenStore" crates/avix-core/src/
grep -r "Escalator"         crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Extend Turn Loop

Complete `src/executor/turn_loop.rs` — the `run_until_complete` method that orchestrates all 7 steps.

---

## Step 2 — Write Tests First

Add to `crates/avix-core/tests/runtime_executor.rs`:

```rust
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::llm_client::{LlmCompleteResponse, StopReason};

// ── Full loop end-to-end ──────────────────────────────────────────────────────

#[tokio::test]
async fn full_loop_read_file_then_end() {
    let (mut executor, mock_llm, mock_fs) = build_test_executor_with_mocks().await;

    // Turn 1: LLM calls fs/read
    mock_llm.push(LlmCompleteResponse {
        content: vec![serde_json::json!({
            "type": "tool_use", "id": "t1", "name": "fs__read",
            "input": {"path": "/test.txt"}
        })],
        stop_reason: StopReason::ToolUse,
        input_tokens: 20, output_tokens: 10,
    });
    mock_fs.on_read("/test.txt", b"hello world");

    // Turn 2: LLM ends
    mock_llm.push(LlmCompleteResponse {
        content: vec![serde_json::json!({"type": "text", "text": "File says: hello world"})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 30, output_tokens: 8,
    });

    let result = executor.run_until_complete("Read test.txt").await.unwrap();
    assert_eq!(result.text, "File says: hello world");
    assert_eq!(mock_llm.call_count(), 2);
}

#[tokio::test]
async fn loop_injects_tool_results_into_messages() {
    let (mut executor, mock_llm, mock_fs) = build_test_executor_with_mocks().await;

    mock_llm.push(LlmCompleteResponse {
        content: vec![serde_json::json!({
            "type": "tool_use", "id": "t1", "name": "fs__read",
            "input": {"path": "/data.txt"}
        })],
        stop_reason: StopReason::ToolUse,
        input_tokens: 10, output_tokens: 5,
    });
    mock_fs.on_read("/data.txt", b"content");

    mock_llm.push(LlmCompleteResponse {
        content: vec![serde_json::json!({"type": "text", "text": "ok"})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 15, output_tokens: 2,
    });

    executor.run_until_complete("Read").await.unwrap();

    let second_call_messages = mock_llm.call_messages(1);
    // Tool result should appear in the message history
    let has_tool_result = second_call_messages.iter().any(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("tool")
        || m.get("content").and_then(|c| c.as_array())
            .map(|a| a.iter().any(|x| x["type"] == "tool_result"))
            .unwrap_or(false)
    });
    assert!(has_tool_result);
}

#[tokio::test]
async fn max_tool_chain_length_enforced() {
    let (mut executor, mock_llm, mock_fs) = build_test_executor_with_max_chain(3).await;

    // LLM keeps calling fs/read indefinitely
    for _ in 0..10 {
        mock_llm.push(LlmCompleteResponse {
            content: vec![serde_json::json!({
                "type": "tool_use", "id": "t1", "name": "fs__read",
                "input": {"path": "/x"}
            })],
            stop_reason: StopReason::ToolUse,
            input_tokens: 5, output_tokens: 2,
        });
        mock_fs.on_read("/x", b"data");
    }

    let err = executor.run_until_complete("Loop").await.unwrap_err();
    assert!(err.to_string().contains("chain") || err.to_string().contains("limit"));
}

#[tokio::test]
async fn token_renewal_transparent_to_loop() {
    let (mut executor, mock_llm, _) = build_test_executor_with_mocks().await;

    // Set token to expire very soon
    executor.set_token_expiry_in(std::time::Duration::from_millis(100));

    mock_llm.push(LlmCompleteResponse {
        content: vec![serde_json::json!({"type": "text", "text": "done"})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5, output_tokens: 2,
    });

    // Loop should complete even though token was about to expire
    let result = executor.run_until_complete("Quick task").await;
    assert!(result.is_ok());
}
```

---

## Step 3 — Implement

Complete `run_until_complete`:

```rust
pub async fn run_until_complete(&mut self, goal: &str) -> Result<TurnResult, AvixError> {
    let mut messages = vec![json!({"role": "user", "content": goal})];
    let mut chain_count = 0;

    loop {
        // Step 1: refresh tool list
        self.flush_tool_changed_events();

        // Step 2: build system prompt
        let system = self.build_system_prompt();

        // Step 3: call llm/complete
        let response = self.llm_complete(&messages, &system).await?;

        // Step 4: check token renewal
        self.maybe_renew_token().await;

        // Step 5: interpret stop reason
        match interpret_stop_reason(&response) {
            TurnAction::ReturnResult(text) => return Ok(TurnResult { text }),
            TurnAction::SummariseContext   => { self.summarise_context(&mut messages); continue; }
            TurnAction::DispatchTools(calls) => {
                chain_count += calls.len();
                if chain_count > self.max_tool_chain_length {
                    return Err(AvixError::ConfigParse(
                        format!("exceeded max tool chain length of {}", self.max_tool_chain_length)
                    ));
                }

                let mut results = Vec::new();
                for call in calls {
                    // Steps 5a–5e
                    validate_tool_call(&self.token, &call, &self.budgets)?;
                    if self.needs_hil_approval(&call.name) {
                        let approval = self.request_hil_approval(&call).await?;
                        if !approval.approved {
                            results.push(self.make_denial_result(&call, &approval));
                            continue;
                        }
                    }
                    let result = if ToolCategory::classify(&call.name) == ToolCategory::AvixBehaviour {
                        self.dispatch_category2(&call).await?
                    } else {
                        self.dispatch_via_router(&call).await?
                    };
                    results.push(result);
                }

                // Step 6: inject results
                for r in results {
                    messages.push(self.format_tool_result_message(&r));
                }
                // Step 7: continue
            }
        }
    }
}
```

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 20+ integration loop tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-18: RuntimeExecutor full turn loop — tool dispatch, injection, chain limit"
```

## Success Criteria

- [ ] Full loop: read file → inject result → end turn works end-to-end
- [ ] Tool result appears in message history for second LLM call
- [ ] `maxToolChainLength` stops the loop with an error
- [ ] Token renewal does not interrupt the loop
- [ ] `run_until_complete` iterates correctly on multi-turn tool use
- [ ] 20+ tests pass, 0 clippy warnings

---
---

