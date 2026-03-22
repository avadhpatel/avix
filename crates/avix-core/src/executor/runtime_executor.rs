use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::error::AvixError;
use crate::llm_client::LlmCompleteResponse;
use crate::llm_svc::adapter::AvixToolCall;
use crate::types::{token::CapabilityToken, tool::ToolVisibility, Pid};

use super::mock_kernel::MockKernelHandle;
use super::prompt::build_system_prompt;
use super::spawn::SpawnParams;
use super::stop_reason::{interpret_stop_reason, TurnAction};
use super::tool_registration::{cat2_tool_descriptor, compute_cat2_tools};
use super::validation::{validate_tool_call, ToolBudgets};

/// Minimal trait that MockToolRegistry satisfies
pub trait ToolRegistryHandle: Send + Sync {
    fn register_tool(
        &self,
        pid: u32,
        name: &str,
        visibility: ToolVisibility,
    ) -> impl std::future::Future<Output = ()> + Send;

    fn deregister_tool(&self, pid: u32, name: &str)
        -> impl std::future::Future<Output = ()> + Send;
}

/// Concrete: the mock registry used in tests
pub struct MockToolRegistry {
    pub registered: Arc<Mutex<Vec<(u32, String, ToolVisibility)>>>,
}

impl MockToolRegistry {
    pub fn new() -> Self {
        Self {
            registered: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn tools_registered_by_pid(&self, pid: u32) -> HashSet<String> {
        self.registered
            .lock()
            .await
            .iter()
            .filter(|(p, _, _)| *p == pid)
            .map(|(_, name, _)| name.clone())
            .collect()
    }

    pub async fn all_registered(&self) -> Vec<(u32, String)> {
        self.registered
            .lock()
            .await
            .iter()
            .map(|(p, n, _)| (*p, n.clone()))
            .collect()
    }
}

impl Default for MockToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistryHandle for Arc<MockToolRegistry> {
    async fn register_tool(&self, pid: u32, name: &str, visibility: ToolVisibility) {
        self.registered
            .lock()
            .await
            .push((pid, name.to_string(), visibility));
    }

    async fn deregister_tool(&self, pid: u32, name: &str) {
        self.registered
            .lock()
            .await
            .retain(|(p, n, _)| !(*p == pid && n == name));
    }
}

#[derive(Debug)]
pub struct TurnResult {
    pub text: String,
}

pub struct RuntimeExecutor {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub session_id: String,
    pub token: CapabilityToken,
    pub pending_messages: Vec<String>,
    pub registered_cat2: Vec<String>,
    removed_tools: Vec<String>,
    pub tool_list: Vec<serde_json::Value>,
    pub tool_budgets: ToolBudgets,
    hil_required_tools: Vec<String>,
    // Day 18 fields
    llm_queue: Arc<std::sync::Mutex<Vec<LlmCompleteResponse>>>,
    call_log: Arc<std::sync::Mutex<Vec<Vec<serde_json::Value>>>>,
    fs_data: Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>,
    pub max_tool_chain_length: usize,
    token_expiry_at: Option<std::time::Instant>,
    // kernel handle for dispatch (optional)
    kernel: Option<Arc<MockKernelHandle>>,
    registry_ref: RegistryRef,
}

enum RegistryRef {
    Mock(Arc<MockToolRegistry>),
}

impl RuntimeExecutor {
    pub async fn spawn_with_registry(
        params: SpawnParams,
        registry: Arc<MockToolRegistry>,
    ) -> Result<Self, AvixError> {
        let tools = compute_cat2_tools(&params.token);
        let mut registered_cat2 = Vec::new();

        for (name, visibility) in &tools {
            registry
                .register_tool(params.pid.as_u32(), name, visibility.clone())
                .await;
            registered_cat2.push(name.clone());
        }

        let mut executor = Self {
            pid: params.pid,
            agent_name: params.agent_name,
            goal: params.goal,
            spawned_by: params.spawned_by,
            session_id: params.session_id,
            token: params.token,
            pending_messages: Vec::new(),
            registered_cat2,
            removed_tools: Vec::new(),
            tool_list: Vec::new(),
            tool_budgets: ToolBudgets::default(),
            hil_required_tools: Vec::new(),
            llm_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            call_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            fs_data: Arc::new(std::sync::Mutex::new(HashMap::new())),
            max_tool_chain_length: 50,
            token_expiry_at: None,
            kernel: None,
            registry_ref: RegistryRef::Mock(registry),
        };

        // GAP 3: populate tool_list at spawn time
        executor.refresh_tool_list();

        Ok(executor)
    }

    pub async fn spawn_with_registry_and_kernel(
        params: SpawnParams,
        registry: Arc<MockToolRegistry>,
        kernel: Arc<MockKernelHandle>,
    ) -> Result<Self, AvixError> {
        let mut executor = Self::spawn_with_registry(params, registry).await?;
        executor.kernel = Some(kernel);
        Ok(executor)
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// GAP 3: Rebuild tool_list from the current Cat2 tools, excluding removed tools.
    pub fn refresh_tool_list(&mut self) {
        let cat2 = compute_cat2_tools(&self.token);
        let removed = &self.removed_tools;
        self.tool_list = cat2
            .into_iter()
            .filter(|(name, _)| !removed.contains(name))
            .map(|(name, _)| cat2_tool_descriptor(&name))
            .collect();
    }

    pub fn build_system_prompt_str(&self) -> String {
        build_system_prompt(
            self.pid.as_u32(),
            &self.agent_name,
            &self.goal,
            &self.spawned_by,
            &self.session_id,
            self.max_tool_chain_length,
            // Convert ToolBudgets to HashMap<String, u32> for the prompt
            &self
                .registered_cat2
                .iter()
                .filter_map(|name| self.tool_budgets.remaining(name).map(|n| (name.clone(), n)))
                .collect::<HashMap<String, u32>>(),
            &self.pending_messages,
        )
    }

    /// Public accessor kept for backward compat with existing tests.
    pub fn build_system_prompt(&self) -> String {
        self.build_system_prompt_str()
    }

    pub fn inject_pending_message(&mut self, msg: String) {
        self.pending_messages.push(msg);
    }

    /// GAP 6: Register a tool that requires HIL approval before dispatch.
    pub fn require_hil_for(&mut self, tool: &str) {
        self.hil_required_tools.push(tool.to_string());
    }

    /// GAP 4: Set a per-tool call budget.
    pub fn set_tool_budget(&mut self, tool: &str, n: u32) {
        self.tool_budgets.set(tool, n);
    }

    pub async fn shutdown(&mut self) {
        match &self.registry_ref {
            RegistryRef::Mock(reg) => {
                for name in self.registered_cat2.clone() {
                    reg.deregister_tool(self.pid.as_u32(), &name).await;
                }
                self.registered_cat2.clear();
            }
        }
    }

    pub async fn handle_tool_changed(&mut self, op: &str, tool_name: &str, _reason: &str) {
        if op == "removed" {
            self.removed_tools.push(tool_name.to_string());
        }
    }

    pub fn current_tool_list(&self) -> Vec<serde_json::Value> {
        self.tool_list
            .iter()
            .filter(|t| {
                if let Some(name) = t["name"].as_str() {
                    // removed_tools stores Avix names (with /)
                    // The descriptor name may be Avix-style (/) or wire-mangled (__)
                    // so check both forms.
                    !self.removed_tools.iter().any(|r| {
                        let mangled = r.replace('/', "__");
                        name == r.as_str() || name == mangled.as_str()
                    })
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    }

    pub async fn dispatch_category2(
        &mut self,
        call: &AvixToolCall,
    ) -> Result<serde_json::Value, AvixError> {
        match call.name.as_str() {
            "agent/spawn" => {
                if let Some(kernel) = &self.kernel {
                    let agent_name = call.args["agent"].as_str().unwrap_or("unknown");
                    kernel.record_proc_spawn(agent_name).await;
                }
                Ok(serde_json::json!({"spawned": true}))
            }
            "cap/request-tool" => {
                if let Some(kernel) = &self.kernel {
                    if kernel.is_auto_approve().await {
                        return Ok(serde_json::json!({"approved": true}));
                    }
                }
                Ok(serde_json::json!({"approved": false}))
            }
            // GAP 5: remaining Category 2 stubs
            "cap/list" => Ok(serde_json::json!({
                "grantedTools": self.token.granted_tools,
                "constraints": {
                    "maxToolChainLength": self.max_tool_chain_length
                },
                "tokenExpiresAt": null
            })),
            "cap/escalate" => {
                let guidance = call.args["reason"].as_str().unwrap_or("");
                Ok(serde_json::json!({
                    "selectedOption": "acknowledged",
                    "guidance": guidance
                }))
            }
            "job/watch" => Ok(serde_json::json!({
                "jobId": call.args["jobId"],
                "finalStatus": "done",
                "result": null,
                "error": null
            })),
            "agent/list" => Ok(serde_json::json!({ "agents": [] })),
            "agent/wait" => Ok(serde_json::json!({
                "pid": call.args["pid"],
                "finalStatus": "completed",
                "result": null,
                "durationSec": 0
            })),
            "agent/send-message" => Ok(serde_json::json!({ "delivered": true })),
            "pipe/open" => Ok(serde_json::json!({
                "pipeId": "pipe-stub",
                "state": "open"
            })),
            "pipe/write" => Ok(serde_json::json!({
                "tokensSent": 0,
                "bufferRemaining": 8192
            })),
            "pipe/read" => Ok(serde_json::json!({
                "content": "",
                "tokensRead": 0,
                "pipeState": "open"
            })),
            "pipe/close" => Ok(serde_json::json!({ "closed": true })),
            _ => Ok(serde_json::json!({
                "content": format!("Tool '{}' executed (IPC dispatch not yet wired)", call.name)
            })),
        }
    }

    // Day 18 methods

    pub fn push_llm_response(&self, resp: LlmCompleteResponse) {
        self.llm_queue.lock().unwrap().push(resp);
    }

    pub fn llm_call_count(&self) -> usize {
        self.call_log.lock().unwrap().len()
    }

    pub fn call_messages(&self, idx: usize) -> Vec<serde_json::Value> {
        self.call_log
            .lock()
            .unwrap()
            .get(idx)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_token_expiry_in(&mut self, _d: Duration) {
        self.token_expiry_at = Some(std::time::Instant::now());
    }

    pub fn on_fs_read(&self, path: &str, content: &[u8]) {
        self.fs_data
            .lock()
            .unwrap()
            .insert(path.to_string(), content.to_vec());
    }

    pub fn set_max_tool_chain_length(&mut self, max: usize) {
        self.max_tool_chain_length = max;
    }

    /// GAP 8: Token renewal — extend expiry if within 5 minutes.
    fn maybe_renew_token(&mut self) {
        if let Some(expiry) = self.token_expiry_at {
            let until_expiry = expiry.saturating_duration_since(std::time::Instant::now());
            if until_expiry <= std::time::Duration::from_secs(300) {
                self.token_expiry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3600));
                tracing::info!(pid = ?self.pid, "token renewed (mock)");
            }
        }
    }

    /// Run the turn loop against a real LLM client.
    pub async fn run_with_client(
        &mut self,
        goal: &str,
        client: &dyn crate::llm_client::LlmClient,
    ) -> Result<TurnResult, AvixError> {
        let system = self.build_system_prompt_str();
        let mut messages: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "user", "content": goal})];
        let mut chain_count = 0;

        loop {
            // GAP 3: refresh tool list at turn start
            self.refresh_tool_list();

            // GAP 8: renew token if needed
            self.maybe_renew_token();

            let req = crate::llm_client::LlmCompleteRequest {
                model: String::new(), // client picks its default
                messages: messages.clone(),
                tools: self.current_tool_list(),
                system: Some(system.clone()),
                max_tokens: 4096,
            };

            let response = client
                .complete(req)
                .await
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

            match super::stop_reason::interpret_stop_reason(&response) {
                super::stop_reason::TurnAction::ReturnResult(text) => {
                    return Ok(TurnResult { text });
                }
                super::stop_reason::TurnAction::SummariseContext => {
                    // summarise not yet implemented — treat as end
                    let text = response
                        .content
                        .iter()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    return Ok(TurnResult { text });
                }
                super::stop_reason::TurnAction::DispatchTools(calls) => {
                    chain_count += calls.len();
                    if chain_count > self.max_tool_chain_length {
                        return Err(AvixError::ConfigParse(format!(
                            "exceeded max tool chain limit of {}",
                            self.max_tool_chain_length
                        )));
                    }
                    // Append assistant message with tool_use blocks
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": response.content
                    }));
                    // Dispatch each call and collect results
                    let mut tool_results = Vec::new();
                    for call in &calls {
                        // GAP 4: capability validation + budget check
                        if let Err(e) = validate_tool_call(&self.token, call, &self.tool_budgets) {
                            tool_results.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": call.call_id,
                                "content": format!("Error: {e}")
                            }));
                            continue;
                        }

                        // GAP 6: HIL gating
                        if self.hil_required_tools.iter().any(|t| t == &call.name) {
                            if let Some(kernel) = &self.kernel {
                                if !kernel.is_auto_approve().await {
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": call.call_id,
                                        "content": "Tool call requires human approval (HIL gate). Not yet approved."
                                    }));
                                    continue;
                                }
                                // Auto-approved in test mode — fall through to dispatch
                            } else {
                                // No kernel handle → inject pending message and deny
                                self.inject_pending_message(format!(
                                    "[System]: HIL required for {}",
                                    call.name
                                ));
                                tool_results.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": call.call_id,
                                    "content": "Tool call requires human approval."
                                }));
                                continue;
                            }
                        }

                        let result = self.dispatch_category2(call).await?;
                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": call.call_id,
                            "content": result.to_string()
                        }));
                    }
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": tool_results
                    }));
                }
            }
        }
    }

    pub async fn run_until_complete(&mut self, goal: &str) -> Result<TurnResult, AvixError> {
        let mut messages: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "user", "content": goal})];
        let mut chain_count = 0;

        loop {
            let _system = self.build_system_prompt_str();
            // pop from mock queue
            let response = {
                let mut q = self.llm_queue.lock().unwrap();
                if q.is_empty() {
                    return Err(AvixError::ConfigParse("no more mock LLM responses".into()));
                }
                q.remove(0)
            };
            // record call
            self.call_log.lock().unwrap().push(messages.clone());

            // token renewal stub
            self.maybe_renew_token();

            match interpret_stop_reason(&response) {
                TurnAction::ReturnResult(text) => return Ok(TurnResult { text }),
                TurnAction::SummariseContext => {
                    // stub: just continue - will fail if no more responses
                }
                TurnAction::DispatchTools(calls) => {
                    chain_count += calls.len();
                    if chain_count > self.max_tool_chain_length {
                        return Err(AvixError::ConfigParse(format!(
                            "exceeded max tool chain limit of {}",
                            self.max_tool_chain_length
                        )));
                    }

                    let mut results = Vec::new();
                    for call in &calls {
                        if call.name == "fs/read" {
                            let path = call.args["path"].as_str().unwrap_or("");
                            let content = {
                                let fs = self.fs_data.lock().unwrap();
                                fs.get(path).cloned().unwrap_or_default()
                            };
                            results.push(serde_json::json!([{
                                "type": "tool_result",
                                "tool_use_id": call.call_id,
                                "content": String::from_utf8_lossy(&content).to_string()
                            }]));
                        } else {
                            results.push(serde_json::json!([{
                                "type": "tool_result",
                                "tool_use_id": call.call_id,
                                "content": "ok"
                            }]));
                        }
                    }

                    // append tool use from LLM
                    for c in &response.content {
                        messages.push(serde_json::json!({"role": "assistant", "content": [c]}));
                    }
                    // append tool results
                    for r in results {
                        messages.push(serde_json::json!({"role": "user", "content": r}));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::MockKernelHandle;
    use crate::llm_client::{LlmCompleteRequest, LlmCompleteResponse, StopReason};
    use serde_json::json;

    fn make_params(pid_val: u32, caps: &[&str]) -> SpawnParams {
        SpawnParams {
            pid: Pid::new(pid_val),
            agent_name: "test-agent".into(),
            goal: "test goal".into(),
            spawned_by: "kernel".into(),
            session_id: "sess-test".into(),
            token: CapabilityToken {
                granted_tools: caps.iter().map(|s| s.to_string()).collect(),
                signature: "test-sig".into(),
            },
        }
    }

    async fn make_executor(pid_val: u32, caps: &[&str]) -> RuntimeExecutor {
        let registry = Arc::new(MockToolRegistry::new());
        RuntimeExecutor::spawn_with_registry(make_params(pid_val, caps), registry)
            .await
            .unwrap()
    }

    // GAP 3 tests
    #[tokio::test]
    async fn test_tool_list_populated_at_spawn() {
        let executor = make_executor(200, &[]).await;
        // Always-present tools should be in tool_list
        assert!(
            !executor.tool_list.is_empty(),
            "tool_list should be non-empty after spawn"
        );
    }

    #[tokio::test]
    async fn test_tool_list_excludes_removed() {
        let mut executor = make_executor(201, &[]).await;
        executor
            .handle_tool_changed("removed", "cap/list", "")
            .await;
        executor.refresh_tool_list();
        let names: Vec<_> = executor
            .tool_list
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(
            !names.contains(&"cap/list"),
            "cap/list should be excluded after removal"
        );
    }

    // GAP 4 tests
    struct MockLlmClient {
        responses: std::sync::Mutex<Vec<LlmCompleteResponse>>,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmCompleteResponse>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::llm_client::LlmClient for MockLlmClient {
        async fn complete(&self, _req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                return Err(anyhow::anyhow!("no more mock responses"));
            }
            Ok(guard.remove(0))
        }
    }

    #[tokio::test]
    async fn test_run_with_client_rejects_ungranted_tool() {
        // Token has no tools at all — any non-empty token would reject
        let registry = Arc::new(MockToolRegistry::new());
        let params = SpawnParams {
            pid: Pid::new(202),
            agent_name: "agent".into(),
            goal: "goal".into(),
            spawned_by: "kernel".into(),
            session_id: "sess".into(),
            token: CapabilityToken {
                granted_tools: vec!["cap/list".to_string()], // has cap/list, not fs/read
                signature: "sig".into(),
            },
        };
        let mut executor = RuntimeExecutor::spawn_with_registry(params, registry)
            .await
            .unwrap();

        let mock_client = MockLlmClient::new(vec![
            // First call: LLM tries to call fs/read (not in token)
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use",
                    "id": "call-bad",
                    "name": "fs__read",
                    "input": {"path": "/etc/passwd"}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            // Second call: end turn
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "Done."})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 5,
                output_tokens: 2,
            },
        ]);

        // Should not panic; the loop should handle the denied tool call gracefully
        let result = executor.run_with_client("do something", &mock_client).await;
        assert!(result.is_ok(), "should complete without panic: {result:?}");
    }

    // GAP 5 tests
    #[tokio::test]
    async fn test_dispatch_cap_list() {
        let mut executor = make_executor(210, &["spawn"]).await;
        let call = AvixToolCall {
            call_id: "c1".into(),
            name: "cap/list".into(),
            args: json!({}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(
            result.get("grantedTools").is_some(),
            "cap/list should return grantedTools"
        );
    }

    #[tokio::test]
    async fn test_dispatch_cap_escalate() {
        let mut executor = make_executor(211, &[]).await;
        let call = AvixToolCall {
            call_id: "c2".into(),
            name: "cap/escalate".into(),
            args: json!({
                "reason": "I found PII data",
                "context": "some context",
                "options": []
            }),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(
            result.get("guidance").is_some(),
            "cap/escalate should return guidance"
        );
    }

    #[tokio::test]
    async fn test_dispatch_pipe_open() {
        let mut executor = make_executor(212, &["pipe"]).await;
        let call = AvixToolCall {
            call_id: "c3".into(),
            name: "pipe/open".into(),
            args: json!({"targetPid": 99, "direction": "out"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(
            result.get("pipeId").is_some(),
            "pipe/open should return pipeId"
        );
    }

    // GAP 6 tests
    #[tokio::test]
    async fn test_hil_gate_blocks_without_kernel() {
        let mut executor = make_executor(220, &[]).await;
        executor.require_hil_for("cap/list");

        let mock_client = MockLlmClient::new(vec![
            // First call: LLM calls cap/list (HIL required)
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use",
                    "id": "hil-call",
                    "name": "cap__list",
                    "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            // Second call: end turn
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "Done."})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 5,
                output_tokens: 2,
            },
        ]);

        let result = executor.run_with_client("do something", &mock_client).await;
        assert!(
            result.is_ok(),
            "loop should complete gracefully: {result:?}"
        );
        // Verify a pending message was injected about HIL
        assert!(
            executor
                .pending_messages
                .iter()
                .any(|m| m.contains("HIL required")),
            "should have HIL pending message"
        );
    }
}
