use avix_core::executor::runtime_executor::MockToolRegistry;
use avix_core::executor::stop_reason::{interpret_stop_reason, TurnAction};
use avix_core::executor::validation::{validate_tool_call, ToolBudgets};
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::kernel::KernelResourceHandler;
use avix_core::llm_client::{LlmCompleteResponse, StopReason};
use avix_core::llm_svc::adapter::AvixToolCall;
use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::types::token::CapabilityToken;
use avix_core::types::{tool::ToolVisibility, Pid};
use serde_json::json;
use std::sync::Arc;

const TEST_KEY: &[u8] = b"test-master-key-32-bytes-padded!";

fn token_with_caps(caps: &[&str]) -> CapabilityToken {
    CapabilityToken::test_token(caps)
}

async fn spawn_with_caps(pid_val: u64, caps: &[&str]) -> (RuntimeExecutor, Arc<MockToolRegistry>) {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::from_u64(pid_val),
        agent_name: "test-agent".into(),
        goal: "do something".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(caps),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
    assert_eq!(executor.pid().as_u64(), 42);
}

#[tokio::test]
async fn spawn_cap_registers_agent_tools() {
    let (_, registry) = spawn_with_caps(
        10,
        &[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
        ],
    )
    .await;
    let tools = registry.tools_registered_by_pid(10).await;
    assert!(tools.contains("agent/spawn"));
    assert!(tools.contains("agent/kill"));
    assert!(tools.contains("agent/list"));
    assert!(tools.contains("agent/wait"));
    assert!(tools.contains("agent/send-message"));
}

#[tokio::test]
async fn pipe_cap_registers_pipe_tools() {
    let (_, registry) =
        spawn_with_caps(11, &["pipe/open", "pipe/write", "pipe/read", "pipe/close"]).await;
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
        pid: Pid::from_u64(14),
        agent_name: "test".into(),
        goal: "test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
            "pipe/open",
            "pipe/write",
            "pipe/read",
            "pipe/close",
        ]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
        pid: Pid::from_u64(15),
        agent_name: "test".into(),
        goal: "test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
        ]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
    let token = CapabilityToken::test_token(&[]);
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
        ex.tools.tool_list = vec![
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
        pid: Pid::from_u64(40),
        agent_name: "orchestrator".into(),
        goal: "spawn subagents".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
        ]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
        pid: Pid::from_u64(42),
        agent_name: "orchestrator".into(),
        goal: "kill subagent".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
        ]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
        pid: Pid::from_u64(41),
        agent_name: "worker".into(),
        goal: "request caps".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
        pid: Pid::from_u64(100),
        agent_name: "test-agent".into(),
        goal: "test goal".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
            "pipe/open",
            "pipe/write",
            "pipe/read",
            "pipe/close",
        ]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
    };
    RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
}

async fn build_test_executor_with_max_chain(max: usize) -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::from_u64(101),
        agent_name: "chain-agent".into(),
        goal: "chain test".into(),
        spawned_by: "kernel".into(),
        session_id: "test-session".into(),
        token: token_with_caps(&[]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
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
        m["content"]
            .as_array()
            .is_some_and(|c| c.iter().any(|item| item["type"] == "tool_result"))
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

// ---- Token expiry tests ----

#[tokio::test]
async fn expired_token_aborts_run_until_complete() {
    let (mut executor, _) = spawn_with_caps(300, &[]).await;
    // Set expiry to 0 seconds — expires_at = Utc::now(), which is already past by loop time.
    executor.set_token_expiry_in(std::time::Duration::ZERO);

    // Push a valid response — but it should never be consumed because expiry fails first
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Should not reach here."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5,
        output_tokens: 2,
    });

    let result = executor.run_until_complete("go").await;
    assert!(result.is_err(), "expired token should abort the run");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("expired"),
        "error should mention expiry: {msg}"
    );
}

#[tokio::test]
async fn cap_list_returns_token_expires_at() {
    let (mut executor, _) = spawn_with_caps(301, &[]).await;
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "c1".into(),
        name: "cap/list".into(),
        args: json!({}),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    let expires_at = result["tokenExpiresAt"].as_str().unwrap_or("");
    assert!(
        !expires_at.is_empty(),
        "cap/list should return a non-null tokenExpiresAt"
    );
    // Should be a valid RFC3339 timestamp
    assert!(
        expires_at.contains('T'),
        "tokenExpiresAt should be an ISO timestamp: {expires_at}"
    );
}

#[tokio::test]
async fn maybe_renew_extends_near_expiry_token() {
    let (mut executor, _) = spawn_with_caps(302, &[]).await;
    // Set token to expire in 2 minutes (within the 5-minute renewal window)
    executor.set_token_expiry_in(std::time::Duration::from_secs(120));
    // Push a response so run_until_complete can proceed
    executor.push_llm_response(LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "Done."})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5,
        output_tokens: 2,
    });
    // Should succeed because maybe_renew_token extends the expiry before the guard runs
    let result = executor.run_until_complete("go").await;
    assert!(
        result.is_ok(),
        "near-expiry token should be auto-renewed: {result:?}"
    );
}

// ---- tool.changed event tests ----

#[tokio::test]
async fn tool_changed_added_re_enables_previously_removed_tool() {
    let (mut executor, _) = spawn_with_caps(320, &[]).await;
    // Remove cap/list — current_tool_list() filters it out dynamically
    executor
        .handle_tool_changed("removed", "cap/list", "")
        .await;
    let names_after_remove: Vec<_> = executor
        .current_tool_list()
        .into_iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        !names_after_remove.contains(&"cap/list".to_string()),
        "cap/list should be absent after removal"
    );

    // Re-add it — current_tool_list() should include it again
    executor.handle_tool_changed("added", "cap/list", "").await;
    let names_after_add: Vec<_> = executor
        .current_tool_list()
        .into_iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        names_after_add.contains(&"cap/list".to_string()),
        "cap/list should be present again after added event"
    );
}

#[tokio::test]
async fn tool_changed_current_tool_list_excludes_removed_immediately() {
    let (mut executor, _) = spawn_with_caps(321, &[]).await;
    let count_before = executor.current_tool_list().len();
    executor
        .handle_tool_changed("removed", "cap/escalate", "")
        .await;
    // current_tool_list() filters removed_tools dynamically — no manual refresh needed
    assert_eq!(
        executor.current_tool_list().len(),
        count_before - 1,
        "current_tool_list() should shrink by 1 immediately after removed event"
    );
}

// ---- cap/escalate pending_messages injection tests ----

#[tokio::test]
async fn cap_escalate_injects_guidance_into_pending_messages() {
    let (mut executor, _) = spawn_with_caps(310, &[]).await;
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "esc-1".into(),
        name: "cap/escalate".into(),
        args: json!({
            "reason": "Found sensitive PII data",
            "context": "user record contains SSN",
            "options": []
        }),
    };
    executor.dispatch_category2(&call).await.unwrap();
    // The guidance should have been injected into pending_messages
    assert_eq!(
        executor.pending_messages.len(),
        1,
        "cap/escalate should inject one pending message"
    );
    assert!(
        executor.pending_messages[0].contains("Found sensitive PII data"),
        "pending message should contain the escalation reason: {:?}",
        executor.pending_messages[0]
    );
}

#[tokio::test]
async fn cap_escalate_returns_guidance_in_response() {
    let (mut executor, _) = spawn_with_caps(311, &[]).await;
    let call = avix_core::llm_svc::adapter::AvixToolCall {
        call_id: "esc-2".into(),
        name: "cap/escalate".into(),
        args: json!({ "reason": "Need human judgment", "context": "", "options": [] }),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert!(
        result.get("guidance").is_some(),
        "response should have guidance field"
    );
    assert!(
        result.get("selectedOption").is_some(),
        "response should have selectedOption field"
    );
}

// ── Resource handler wiring tests ──────────────────────────────────────────

async fn spawn_with_signed_token(
    pid_val: u64,
    tools: &[&str],
) -> (RuntimeExecutor, Arc<MockToolRegistry>) {
    let registry = Arc::new(MockToolRegistry::new());
    let token = avix_core::types::token::CapabilityToken::mint(
        tools.iter().map(|s| s.to_string()).collect(),
        None,
        3600,
        TEST_KEY,
    );
    let params = SpawnParams {
        pid: Pid::from_u64(pid_val),
        agent_name: "test-agent".into(),
        goal: "do something".into(),
        spawned_by: "kernel".into(),
        session_id: "session".into(),
        token,
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, Arc::clone(&registry))
        .await
        .unwrap();
    (executor, registry)
}

#[tokio::test]
async fn cap_request_tool_returns_denied_via_resource_handler() {
    // KernelResourceHandler always denies tool grants (HIL required)
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let (executor, _reg) = spawn_with_signed_token(400, &["cap/request-tool"]).await;
    let mut executor = executor.with_resource_handler(handler);

    let call = AvixToolCall {
        call_id: "rh-1".into(),
        name: "cap/request-tool".into(),
        args: json!({ "tool": "send_email", "reason": "notify user" }),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert_eq!(
        result["approved"],
        json!(false),
        "handler should deny tool grants (HIL required)"
    );
    assert_eq!(result["tool"], json!("send_email"));
}

#[tokio::test]
async fn token_renewal_via_resource_handler_updates_token() {
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let (executor, _reg) = spawn_with_signed_token(401, &["fs/read"]).await;
    let mut executor = executor.with_resource_handler(handler);

    // Force the token close to expiry so maybe_renew_token fires
    executor.token.expires_at = chrono::Utc::now() + chrono::Duration::minutes(2);
    let old_expires_at = executor.token.expires_at;

    // Simulate what run_until_complete does at turn start
    // We call maybe_renew_token indirectly by pushing a done response and running
    let response = LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": "done"})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 5,
        output_tokens: 2,
    };
    executor.push_llm_response(response);
    executor.run_until_complete("test goal").await.unwrap();

    assert!(
        executor.token.expires_at > old_expires_at,
        "token expiry should have been extended by resource handler renewal"
    );
    assert!(
        executor.token.verify_signature(TEST_KEY),
        "renewed token must carry a valid HMAC signature"
    );
}

// ── pipe/open ResourceRequest + VFS record tests ───────────────────────────

#[tokio::test]
async fn pipe_open_via_resource_handler_writes_proc_entry() {
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let vfs = Arc::new(VfsRouter::new());
    let (executor, _reg) = spawn_with_signed_token(500, &["pipe/open"]).await;
    let mut executor = executor
        .with_resource_handler(handler)
        .with_vfs(Arc::clone(&vfs));

    let call = AvixToolCall {
        call_id: "pipe-1".into(),
        name: "pipe/open".into(),
        args: json!({ "targetPid": 99, "direction": "out", "bufferTokens": 8192 }),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert!(
        result.get("pipeId").is_some(),
        "pipe/open should return a pipeId"
    );
    let pipe_id = result["pipeId"].as_str().unwrap();

    // The VFS entry must exist at /proc/<pid>/pipes/<pipeId>.yaml
    let path = VfsPath::parse(&format!("/proc/{}/pipes/{}.yaml", 500, pipe_id)).unwrap();
    assert!(
        vfs.exists(&path).await,
        "VFS entry /proc/500/pipes/{pipe_id}.yaml should exist after pipe/open"
    );
}

#[tokio::test]
async fn pipe_open_proc_entry_contains_pipe_metadata() {
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let vfs = Arc::new(VfsRouter::new());
    let (executor, _reg) = spawn_with_signed_token(501, &["pipe/open"]).await;
    let mut executor = executor
        .with_resource_handler(handler)
        .with_vfs(Arc::clone(&vfs));

    let call = AvixToolCall {
        call_id: "pipe-2".into(),
        name: "pipe/open".into(),
        args: json!({ "targetPid": 77, "direction": "bidirectional", "bufferTokens": 4096 }),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    let pipe_id = result["pipeId"].as_str().unwrap();

    let path = VfsPath::parse(&format!("/proc/{}/pipes/{}.yaml", 501, pipe_id)).unwrap();
    let raw = vfs.read(&path).await.expect("VFS entry should be readable");
    let content = String::from_utf8(raw).unwrap();
    assert!(
        content.contains("target_pid") || content.contains("targetPid"),
        "entry should include target_pid"
    );
    assert!(content.contains("77"), "entry should include target pid 77");
}

#[tokio::test]
async fn pipe_open_without_handler_returns_stub() {
    // Without a handler, pipe/open returns the old stub response
    let (mut executor, _reg) = spawn_with_caps(502, &["pipe/open"]).await;
    let call = AvixToolCall {
        call_id: "pipe-stub".into(),
        name: "pipe/open".into(),
        args: json!({ "targetPid": 10, "direction": "out" }),
    };
    let result = executor.dispatch_category2(&call).await.unwrap();
    assert!(
        result.get("pipeId").is_some(),
        "stub should still return pipeId"
    );
}

// ── Finding B: VFS writes at agent spawn ─────────────────────────────────────

#[tokio::test]
async fn spawn_writes_status_yaml_to_vfs() {
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let vfs = Arc::new(VfsRouter::new());
    let (executor, _reg) = spawn_with_signed_token(600, &["fs/read", "llm/complete"]).await;
    let executor = executor
        .with_resource_handler(handler)
        .with_vfs(Arc::clone(&vfs));
    executor.init_proc_files().await;

    let path = VfsPath::parse("/proc/600/status.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/600/status.yaml must exist after spawn when VFS is attached"
    );
}

#[tokio::test]
async fn spawn_status_yaml_contains_pid_and_name() {
    let vfs = Arc::new(VfsRouter::new());
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::from_u64(601),
        agent_name: "my-researcher".into(),
        goal: "do research".into(),
        spawned_by: "alice".into(),
        session_id: "sess-601".into(),
        token: CapabilityToken::test_token(&["fs/read"]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));
    executor.init_proc_files().await;

    let raw = vfs
        .read(&VfsPath::parse("/proc/601/status.yaml").unwrap())
        .await
        .unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("601"), "status.yaml must contain pid 601");
    assert!(
        text.contains("my-researcher"),
        "status.yaml must contain agent name"
    );
    assert!(text.contains("alice"), "status.yaml must contain spawnedBy");
    assert!(
        text.contains("running"),
        "status.yaml must show status: running"
    );
}

#[tokio::test]
async fn spawn_writes_resolved_yaml_to_vfs() {
    let vfs = Arc::new(VfsRouter::new());
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::from_u64(602),
        agent_name: "writer".into(),
        goal: "write report".into(),
        spawned_by: "kernel".into(),
        session_id: "sess-602".into(),
        token: CapabilityToken::test_token(&["fs/read", "fs/write"]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            atp_session_id: String::new(),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));
    executor.init_proc_files().await;

    let path = VfsPath::parse("/proc/602/resolved.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/602/resolved.yaml must exist after spawn when VFS is attached"
    );
    let raw = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(
        text.contains("fs/read"),
        "resolved.yaml must list granted tools"
    );
    assert!(
        text.contains("fs/write"),
        "resolved.yaml must list all granted tools"
    );
}

#[tokio::test]
async fn spawn_without_vfs_does_not_panic() {
    let (executor, _reg) = spawn_with_caps(603, &["fs/read"]).await;
    assert_eq!(executor.pid().as_u64(), 603);
}
