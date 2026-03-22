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
use super::tool_registration::compute_cat2_tools;

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
    pub token: CapabilityToken,
    pub pending_messages: Vec<String>,
    pub registered_cat2: Vec<String>,
    removed_tools: Vec<String>,
    pub tool_list: Vec<serde_json::Value>,
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

        Ok(Self {
            pid: params.pid,
            agent_name: params.agent_name,
            goal: params.goal,
            spawned_by: params.spawned_by,
            token: params.token,
            pending_messages: Vec::new(),
            registered_cat2,
            removed_tools: Vec::new(),
            tool_list: Vec::new(),
            llm_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            call_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            fs_data: Arc::new(std::sync::Mutex::new(HashMap::new())),
            max_tool_chain_length: 50,
            token_expiry_at: None,
            kernel: None,
            registry_ref: RegistryRef::Mock(registry),
        })
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

    pub fn build_system_prompt(&self) -> String {
        build_system_prompt(
            self.pid.as_u32(),
            &self.agent_name,
            &self.goal,
            &self.pending_messages,
        )
    }

    pub fn inject_pending_message(&mut self, msg: String) {
        self.pending_messages.push(msg);
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
                    !self.removed_tools.iter().any(|r| {
                        let mangled = r.replace('/', "__");
                        name == mangled
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
            _ => Ok(serde_json::json!({"ok": true})),
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

    fn maybe_renew_token(&mut self) {
        // Token renewal is transparent — no-op in tests
    }

    /// Run the turn loop against a real LLM client.
    pub async fn run_with_client(
        &mut self,
        goal: &str,
        client: &dyn crate::llm_client::LlmClient,
    ) -> Result<TurnResult, AvixError> {
        let system = self.build_system_prompt();
        let mut messages: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "user", "content": goal})];
        let mut chain_count = 0;

        loop {
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
            let _system = self.build_system_prompt();
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
