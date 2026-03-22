use avix_core::executor::runtime_executor::MockToolRegistry;
use avix_core::executor::stop_reason::{interpret_stop_reason, TurnAction};
use avix_core::executor::validation::{validate_tool_call, ToolBudgets};
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::llm_client::{LlmCompleteResponse, StopReason};
use avix_core::llm_svc::adapter::AvixToolCall;
use avix_core::types::token::CapabilityToken;
use avix_core::types::{tool::ToolVisibility, Pid};
use serde_json::json;
use std::sync::Arc;

fn token_with_caps(caps: &[&str]) -> CapabilityToken {
    CapabilityToken {
        granted_tools: caps.iter().map(|s| s.to_string()).collect(),
        signature: "test-sig".into(),
    }
}

async fn spawn_with_caps(pid_val: u32, caps: &[&str]) -> (RuntimeExecutor, Arc<MockToolRegistry>) {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(pid_val),
        agent_name: "test-agent".into(),
        goal: "do something".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(caps),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, Arc::clone(&registry))
        .await
        .unwrap();
    (executor, registry)
}

// ---- Day 15 tests ----

#[tokio::test]
async fn executor_spawns_with_correct_pid_and_token() {
    let (executor, _registry) = spawn_with_caps(42, &["agent/spawn"]).await;
    assert_eq!(executor.pid().as_u32(), 42);
}

#[tokio::test]
async fn spawn_cap_registers_agent_tools() {
    let (_, registry) = spawn_with_caps(10, &["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"]).await;
    let tools = registry.tools_registered_by_pid(10).await;
    assert!(tools.contains("agent/spawn"));
    assert!(tools.contains("agent/kill"));
    assert!(tools.contains("agent/list"));
    assert!(tools.contains("agent/wait"));
    assert!(tools.contains("agent/send-message"));
}

#[tokio::test]
async fn pipe_cap_registers_pipe_tools() {
    let (_, registry) = spawn_with_caps(11, &["pipe/open", "pipe/write", "pipe/read", "pipe/close"]).await;
    let tools = registry.tools_registered_by_pid(11).await;
    assert!(tools.contains("pipe/open"));
    assert!(tools.contains("pipe/write"));
    assert!(tools.contains("pipe/read"));
    assert!(tools.contains("pipe/close"));
}

#[tokio::test]
async fn always_present_tools_registered_regardless_of_caps() {
    let (_, registry) = spawn_with_caps(12, &[]).await;
    let tools = registry.tools_registered_by_pid(12).await;
    assert!(tools.contains("cap/request-tool"));
    assert!(tools.contains("cap/escalate"));
    assert!(tools.contains("cap/list"));
    assert!(tools.contains("job/watch"));
}

#[tokio::test]
async fn absent_spawn_cap_does_not_register_agent_tools() {
    let (_, registry) = spawn_with_caps(13, &[]).await;
    let tools = registry.tools_registered_by_pid(13).await;
    assert!(!tools.contains("agent/spawn"));
}

#[tokio::test]
async fn shutdown_deregisters_all_category2_tools() {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(14),
        agent_name: "test".into(),
        goal: "test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message", "pipe/open", "pipe/write", "pipe/read", "pipe/close"]),
    };
    let mut executor = RuntimeExecutor::spawn_with_registry(params, Arc::clone(&registry))
        .await
        .unwrap();

    let before = registry.tools_registered_by_pid(14).await;
    assert!(!before.is_empty());

    executor.shutdown().await;
    let after = registry.tools_registered_by_pid(14).await;
    assert!(after.is_empty());
}

#[tokio::test]
async fn category2_tools_registered_with_user_visibility() {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(15),
        agent_name: "test".into(),
        goal: "test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"]),
    };
    RuntimeExecutor::spawn_with_registry(params, Arc::clone(&registry))
        .await
        .unwrap();

    // All Cat2 tools are scoped to the owning user (spawned_by = "kernel")
    let all = registry.registered.lock().await;
    let agent_tools: Vec<_> = all.iter().filter(|(p, _, _)| *p == 15).collect();
    assert!(!agent_tools.is_empty());
    for (_, _, visibility) in &agent_tools {
        assert_eq!(*visibility, ToolVisibility::User("kernel".to_string()));
    }
}

#[tokio::test]
async fn system_prompt_block1_contains_identity() {
    let (executor, _) = spawn_with_caps(20, &[]).await;
    let prompt = executor.build_system_prompt();
    assert!(prompt.contains("test-agent"));
    assert!(prompt.contains("PID: 20"));
    assert!(prompt.contains("do something"));
}

#[tokio::test]
async fn system_prompt_block4_empty_without_pending_messages() {
    let (executor, _) = spawn_with_caps(21, &[]).await;
    let prompt = executor.build_system_prompt();
    assert!(!prompt.contains("Pending Instructions"));
}

#[tokio::test]
async fn system_prompt_block4_shows_pending_messages() {
    let (mut executor, _) = spawn_with_caps(22, &[]).await;
    executor.inject_pending_message("You have a new task from the operator.".into());
    let prompt = executor.build_system_prompt();
    assert!(prompt.contains("Pending Instructions"));
    assert!(prompt.contains("You have a new task from the operator."));
}

// ---- Day 16 tests ----

#[test]
fn interpret_stop_reason_end_turn_returns_text() {
    let resp = LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Done."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 10,
        output_tokens: 5,
    };
    match interpret_stop_reason(&resp) {
        TurnAction::ReturnResult(text) => assert_eq!(text, "Done."),
        _ => panic!("expected ReturnResult"),
    }
}

#[test]
fn interpret_stop_reason_tool_use_dispatches_tools() {
    let resp = LlmCompleteResponse {
        content: vec![json!({
            "type": "tool_use",
            "id": "call-1",
            "name": "fs__read",
            "input": {"path": "/etc/passwd"}
        })],
        stop_reason: StopReason::ToolUse,
        input_tokens: 10,
        output_tokens: 5,
    };
    match interpret_stop_reason(&resp) {
        TurnAction::DispatchTools(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "fs/read");
            assert_eq!(calls[0].call_id, "call-1");
        }
        _ => panic!("expected DispatchTools"),
    }
}

#[test]
fn interpret_stop_reason_max_tokens_summarises() {
    let resp = LlmCompleteResponse {
        content: vec![],
        stop_reason: StopReason::MaxTokens,
        input_tokens: 9999,
        output_tokens: 1,
    };
    match interpret_stop_reason(&resp) {
        TurnAction::SummariseContext => {}
        _ => panic!("expected SummariseContext"),
    }
}

#[test]
fn tool_call_rejected_when_not_in_token() {
    let token = token_with_caps(&["fs/read"]);
    let call = AvixToolCall {
        call_id: "c1".into(),
        name: "send_email".into(),
        args: json!({}),
    };
    let mut budgets = ToolBudgets::default();
    assert!(validate_tool_call(&token, &call, &mut budgets).is_err());
}

#[test]
fn tool_call_rejected_when_budget_zero() {
    let token = token_with_caps(&["send_email"]);
    let call = AvixToolCall {
        call_id: "c2".into(),
        name: "send_email".into(),
        args: json!({}),
    };
    let mut budgets = ToolBudgets::default();
    budgets.set("send_email", 0);
    assert!(validate_tool_call(&token, &call, &mut budgets).is_err());
}

#[test]
fn tool_call_allowed_when_budget_positive() {
    let token = token_with_caps(&["send_email"]);
    let call = AvixToolCall {
        call_id: "c3".into(),
        name: "send_email".into(),
        args: json!({}),
    };
    let mut budgets = ToolBudgets::default();
    budgets.set("send_email", 3);
    assert!(validate_tool_call(&token, &call, &mut budgets).is_ok());
    // Budget should be decremented after a successful call
    assert_eq!(budgets.remaining("send_email"), Some(2));
}

#[test]
fn tool_call_allowed_when_token_is_empty() {
    // Empty granted_tools = no restriction
    let token = CapabilityToken {
        granted_tools: vec![],
        signature: "sig".into(),
    };
    let call = AvixToolCall {
        call_id: "c4".into(),
        name: "anything".into(),
        args: json!({}),
    };
    let mut budgets = ToolBudgets::default();
    assert!(validate_tool_call(&token, &call, &mut budgets).is_ok());
}

#[test]
fn always_present_tools_bypass_capability_check() {
    // Token grants only one specific tool but always-present tools must still pass
    let token = token_with_caps(&["fs/read"]);
    for tool_name in &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"] {
        let call = AvixToolCall {
            call_id: "c-ap".into(),
            name: tool_name.to_string(),
            args: json!({}),
        };
        let mut budgets = ToolBudgets::default();
        assert!(
            validate_tool_call(&token, &call, &mut budgets).is_ok(),
            "always-present tool {tool_name} should not be blocked by capability check"
        );
    }
}

#[test]
fn tool_budget_decrements_to_zero_then_rejects() {
    let token = token_with_caps(&["send_email"]);
    let call = AvixToolCall {
        call_id: "c5".into(),
        name: "send_email".into(),
        args: json!({}),
    };
    let mut budgets = ToolBudgets::default();
    budgets.set("send_email", 2);

    // First call: budget 2 → 1
    assert!(validate_tool_call(&token, &call, &mut budgets).is_ok());
    assert_eq!(budgets.remaining("send_email"), Some(1));

    // Second call: budget 1 → 0
    assert!(validate_tool_call(&token, &call, &mut budgets).is_ok());
    assert_eq!(budgets.remaining("send_email"), Some(0));

    // Third call: budget 0 → rejected
    assert!(validate_tool_call(&token, &call, &mut budgets).is_err());
}

#[tokio::test]
async fn tool_list_excludes_removed_tool_after_changed_event() {
    let mut executor = {
        let (mut ex, _) = spawn_with_caps(30, &[]).await;
        ex.tool_list = vec![
            json!({"name": "fs__read", "description": "Read a file"}),
            json!({"name": "fs__write", "description": "Write a file"}),
        ];
        ex
    };
    executor
        .handle_tool_changed("removed", "fs/write", "service shutdown")
        .await;
    let list = executor.current_tool_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "fs__read");
}

#[tokio::test]
async fn agent_spawn_translates_to_kernel_proc_spawn() {
    use avix_core::executor::MockKernelHandle;

    let registry = Arc::new(MockToolRegistry::new());
    let kernel = Arc::new(MockKernelHandle::new());
    let params = SpawnParams {
        pid: Pid::new(40),
        agent_name: "orchestrator".into(),
        goal: "spawn subagents".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"]),
    };
    let mut executor =
        RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, Arc::clone(&kernel))
            .await
            .unwrap();

    let call = AvixToolCall {
        call_id: "spawn-1".into(),
        name: "agent/spawn".into(),
        args: json!({"agent": "researcher"}),
    };
    executor.dispatch_category2(&call).await.unwrap();
    assert!(kernel.received_proc_spawn("researcher").await);
}

#[tokio::test]
async fn agent_kill_records_in_kernel() {
    use avix_core::executor::MockKernelHandle;

    let registry = Arc::new(MockToolRegistry::new());
    let kernel = Arc::new(MockKernelHandle::new());
    let params = SpawnParams {
        pid: Pid::new(42),
        agent_name: "orchestrator".into(),
        goal: "kill subagent".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"]),
    };
    let mut executor =
        RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, Arc::clone(&kernel))
            .await
            .unwrap();

    let call = AvixToolCall {
        call_id: "kill-1".into(),
        name: "agent/kill".into(),
        args: json!({"pid": 55, "reason": "task complete"}),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert_eq!(result["killed"], true);
    assert!(kernel.received_proc_kill(55).await);
}

#[tokio::test]
async fn cap_request_tool_triggers_resource_request() {
    use avix_core::executor::MockKernelHandle;

    let registry = Arc::new(MockToolRegistry::new());
    let kernel = Arc::new(MockKernelHandle::new());
    kernel.auto_approve_resource_request().await;

    let params = SpawnParams {
        pid: Pid::new(41),
        agent_name: "worker".into(),
        goal: "request caps".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[]),
    };
    let mut executor =
        RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, Arc::clone(&kernel))
            .await
            .unwrap();

    let call = AvixToolCall {
        call_id: "req-1".into(),
        name: "cap/request-tool".into(),
        args: json!({"tool": "fs/write", "reason": "need to write output"}),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert_eq!(result["approved"], true);
}

// ---- Day 18 tests ----

async fn build_test_executor_with_mocks() -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(100),
        agent_name: "test-agent".into(),
        goal: "test goal".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message", "pipe/open", "pipe/write", "pipe/read", "pipe/close"]),
    };
    RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
}

async fn build_test_executor_with_max_chain(max: usize) -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(101),
        agent_name: "chain-agent".into(),
        goal: "chain test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[]),
    };
    let mut executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap();
    executor.set_max_tool_chain_length(max);
    executor
}

#[tokio::test]
async fn run_until_complete_end_turn_returns_text() {
    let mut executor = build_test_executor_with_mocks().await;
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "The answer is 42."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 10,
        output_tokens: 5,
    });
    let result = executor
        .run_until_complete("What is the answer?")
        .await
        .unwrap();
    assert_eq!(result.text, "The answer is 42.");
}

#[tokio::test]
async fn run_until_complete_tool_call_then_end_turn() {
    let mut executor = build_test_executor_with_mocks().await;
    // First response: tool call
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({
            "type": "tool_use",
            "id": "tc-1",
            "name": "fs__read",
            "input": {"path": "/tmp/data.txt"}
        })],
        stop_reason: StopReason::ToolUse,
        input_tokens: 10,
        output_tokens: 5,
    });
    // Second response: final answer
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "File content processed."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 20,
        output_tokens: 10,
    });

    executor.on_fs_read("/tmp/data.txt", b"hello world");
    let result = executor.run_until_complete("Read the file").await.unwrap();
    assert_eq!(result.text, "File content processed.");
    assert_eq!(executor.llm_call_count(), 2);
}

#[tokio::test]
async fn run_until_complete_initial_goal_is_first_message() {
    let mut executor = build_test_executor_with_mocks().await;
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Done."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5,
        output_tokens: 2,
    });

    let _ = executor.run_until_complete("Analyze data").await.unwrap();
    let msgs = executor.call_messages(0);
    assert_eq!(msgs[0]["content"], "Analyze data");
}

#[tokio::test]
async fn run_until_complete_tool_results_appended_to_messages() {
    let mut executor = build_test_executor_with_mocks().await;
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({
            "type": "tool_use",
            "id": "tc-2",
            "name": "fs__read",
            "input": {"path": "/config.yaml"}
        })],
        stop_reason: StopReason::ToolUse,
        input_tokens: 10,
        output_tokens: 5,
    });
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Config loaded."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 20,
        output_tokens: 8,
    });

    executor.on_fs_read("/config.yaml", b"key: value");
    executor.run_until_complete("Load config").await.unwrap();

    // Second LLM call messages should include tool result
    let msgs = executor.call_messages(1);
    let has_tool_result = msgs.iter().any(|m| {
        m["content"].as_array().map_or(false, |c| {
            c.iter().any(|item| item["type"] == "tool_result")
        })
    });
    assert!(has_tool_result);
}

#[tokio::test]
async fn run_until_complete_exceeds_max_chain_returns_error() {
    let mut executor = build_test_executor_with_max_chain(2).await;
    // 3 tool calls in succession
    for _ in 0..3 {
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({
                "type": "tool_use",
                "id": "tc-chain",
                "name": "fs__read",
                "input": {"path": "/file"}
            })],
            stop_reason: StopReason::ToolUse,
            input_tokens: 5,
            output_tokens: 2,
        });
    }
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Done."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5,
        output_tokens: 2,
    });

    let result = executor.run_until_complete("chain test").await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("max tool chain limit"));
}

#[tokio::test]
async fn run_until_complete_stop_sequence_returns_text() {
    let mut executor = build_test_executor_with_mocks().await;
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Stopping here."})],
        stop_reason: StopReason::StopSequence,
        input_tokens: 10,
        output_tokens: 3,
    });
    let result = executor.run_until_complete("test").await.unwrap();
    assert_eq!(result.text, "Stopping here.");
}
