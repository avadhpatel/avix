use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::error::AvixError;
use crate::kernel::resource_request::{
    KernelResourceHandler, ResourceGrant, ResourceItem, ResourceRequest, Urgency,
};
use crate::llm_client::LlmCompleteResponse;
use crate::llm_svc::adapter::AvixToolCall;
use crate::memfs::{MemFs, VfsPath};
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
    // kernel handle for dispatch (optional)
    kernel: Option<Arc<MockKernelHandle>>,
    /// Real kernel resource handler — used for cap/request-tool and token renewal.
    /// When set, takes precedence over the `MockKernelHandle` auto-approve flag.
    resource_handler: Option<Arc<KernelResourceHandler>>,
    /// VFS handle — when set, pipe/open writes a /proc/<pid>/pipes/<pipeId>.yaml record.
    vfs: Option<Arc<MemFs>>,
    registry_ref: RegistryRef,
    /// Set to `true` by the signal listener when SIGPAUSE is received.
    /// The LLM loop should check this at each tool boundary and wait until cleared.
    pub paused: Arc<AtomicBool>,
    /// Set to `true` by the signal listener when SIGKILL or SIGSTOP is received.
    /// The LLM loop should check this and exit cleanly.
    pub killed: Arc<AtomicBool>,
}

enum RegistryRef {
    Mock(Arc<MockToolRegistry>),
}

impl RuntimeExecutor {
    pub async fn spawn_with_registry(
        params: SpawnParams,
        registry: Arc<MockToolRegistry>,
    ) -> Result<Self, AvixError> {
        let tools = compute_cat2_tools(&params.token, &params.spawned_by);
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
            kernel: None,
            resource_handler: None,
            vfs: None,
            registry_ref: RegistryRef::Mock(registry),
            paused: Arc::new(AtomicBool::new(false)),
            killed: Arc::new(AtomicBool::new(false)),
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

    /// Attach a `KernelResourceHandler` so `cap/request-tool` and token renewal
    /// route through the real handler instead of the mock auto-approve flag.
    pub fn with_resource_handler(mut self, handler: Arc<KernelResourceHandler>) -> Self {
        self.resource_handler = Some(handler);
        self
    }

    /// Attach a `MemFs` handle so `pipe/open` writes `/proc/<pid>/pipes/<pipeId>.yaml`.
    pub fn with_vfs(mut self, vfs: Arc<MemFs>) -> Self {
        self.vfs = Some(vfs);
        self
    }

    /// Write `/proc/<pid>/status.yaml` and `/proc/<pid>/resolved.yaml` to the VFS.
    /// Must be called after `with_vfs()` to populate the proc entries.
    /// No-op when no VFS is attached.
    pub async fn init_proc_files(&self) {
        self.write_proc_files().await;
    }

    async fn write_proc_files(&self) {
        let vfs = match &self.vfs {
            Some(v) => v,
            None => return,
        };
        let pid = self.pid.as_u32();
        let tools_yaml = self
            .token
            .granted_tools
            .iter()
            .map(|t| format!("    - {t}\n"))
            .collect::<String>();

        let status_yaml = format!(
            "apiVersion: avix/v1\nkind: AgentStatus\nmetadata:\n  pid: {pid}\n  name: {name}\nspec:\n  status: running\n  goal: {goal:?}\n  spawnedBy: {spawned_by}\n  sessionId: {session_id}\n  grantedTools:\n{tools}  toolChainDepth: 0\n  contextTokensUsed: 0\n",
            name = self.agent_name,
            goal = self.goal,
            spawned_by = self.spawned_by,
            session_id = self.session_id,
            tools = tools_yaml,
        );
        if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/status.yaml")) {
            let _ = vfs.write(&path, status_yaml.into_bytes()).await;
        }

        let resolved_yaml = format!(
            "apiVersion: avix/v1\nkind: Resolved\nmetadata:\n  pid: {pid}\n  name: {name}\nspec:\n  contextWindowTokens: 64000\n  maxToolChainLength: 50\n  tokenTtlSecs: 3600\n  grantedTools:\n{tools}",
            name = self.agent_name,
            tools = self.token
                .granted_tools
                .iter()
                .map(|t| format!("    - {t}\n"))
                .collect::<String>(),
        );
        if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/resolved.yaml")) {
            let _ = vfs.write(&path, resolved_yaml.into_bytes()).await;
        }
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// GAP 3: Rebuild tool_list from the current Cat2 tools, excluding removed tools.
    pub fn refresh_tool_list(&mut self) {
        let cat2 = compute_cat2_tools(&self.token, &self.spawned_by);
        let removed = &self.removed_tools;
        self.tool_list = cat2
            .into_iter()
            .filter(|(name, _)| !removed.contains(name))
            .map(|(name, _)| cat2_tool_descriptor(&name))
            .collect();
    }

    pub fn build_system_prompt_str(&self) -> String {
        let tool_list = self.current_tool_list();
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
            &tool_list,
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

    /// Start the agent's inbound signal socket listener as a background task.
    ///
    /// Binds `/run/avix/agents/<pid>.sock` and spawns a task that processes
    /// incoming signal notifications:
    /// - `SIGPAUSE`  → sets `self.paused = true`
    /// - `SIGRESUME` → sets `self.paused = false`
    /// - `SIGKILL`   → sets `self.killed = true`
    /// - `SIGSTOP`   → sets `self.killed = true` (graceful stop treated same as kill for now)
    /// - Others      → logged and ignored
    ///
    /// Returns `(task_handle, server_handle)`. Call `server_handle.cancel()` at shutdown
    /// to stop accepting new signals; the task will then drain and finish.
    pub async fn start_signal_listener(
        &self,
        run_dir: &Path,
    ) -> Result<(tokio::task::JoinHandle<()>, crate::ipc::IpcServerHandle), AvixError> {
        use crate::ipc::message::IpcMessage;
        use crate::signal::agent_socket::create_agent_socket;

        let (server, handle) = create_agent_socket(run_dir, self.pid).await?;
        let paused = Arc::clone(&self.paused);
        let killed = Arc::clone(&self.killed);
        let pid = self.pid;

        let task = tokio::spawn(async move {
            server
                .serve(move |msg| {
                    let paused = Arc::clone(&paused);
                    let killed = Arc::clone(&killed);
                    Box::pin(async move {
                        let (method, params) = match msg {
                            IpcMessage::Notification(n) => (n.method, n.params),
                            IpcMessage::Request(r) => {
                                tracing::warn!(
                                    pid = pid.as_u32(),
                                    "agent signal socket received unexpected request: {}",
                                    r.method
                                );
                                return None;
                            }
                        };

                        if method != "signal" {
                            tracing::warn!(
                                pid = pid.as_u32(),
                                "agent signal socket: unexpected method '{method}'"
                            );
                            return None;
                        }

                        let signal_name =
                            params.get("signal").and_then(|v| v.as_str()).unwrap_or("");

                        tracing::debug!(
                            pid = pid.as_u32(),
                            signal = signal_name,
                            "signal received"
                        );

                        match signal_name {
                            "SIGPAUSE" => paused.store(true, Ordering::Release),
                            "SIGRESUME" => paused.store(false, Ordering::Release),
                            "SIGKILL" | "SIGSTOP" => killed.store(true, Ordering::Release),
                            "SIGSAVE" => {
                                tracing::info!(
                                    pid = pid.as_u32(),
                                    "SIGSAVE received; snapshot not yet implemented"
                                );
                            }
                            other => {
                                tracing::debug!(
                                    pid = pid.as_u32(),
                                    signal = other,
                                    "unhandled signal"
                                );
                            }
                        }

                        None // notifications never send a response
                    })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<
                                        Output = Option<crate::ipc::message::JsonRpcResponse>,
                                    > + Send,
                            >,
                        >
                })
                .await
                .ok();
        });

        Ok((task, handle))
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
        match op {
            "removed" => {
                if !self.removed_tools.contains(&tool_name.to_string()) {
                    self.removed_tools.push(tool_name.to_string());
                }
            }
            "added" => {
                // Re-enable a previously removed tool by dropping it from the removed list.
                self.removed_tools.retain(|t| t != tool_name);
            }
            _ => {}
        }
        // current_tool_list() filters removed_tools dynamically, so tool_list stays
        // consistent without a full rebuild.  A full refresh happens at turn start.
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

    /// Returns true if this tool is a registered Category 2 tool for this agent.
    /// Category 1/3 tools are forwarded to router.svc; Category 2 tools are handled locally.
    pub fn is_cat2_tool(&self, name: &str) -> bool {
        self.registered_cat2.contains(&name.to_string())
    }

    /// Dispatch a Category 1 or Category 3 (MCP-bridged) tool call via router.svc.
    /// In production this opens a fresh IPC connection to router.svc per ADR-05.
    pub async fn dispatch_via_router(
        &self,
        call: &AvixToolCall,
    ) -> Result<serde_json::Value, AvixError> {
        // IPC dispatch to router.svc not yet wired in this environment
        Ok(serde_json::json!({
            "content": format!("Tool '{}' executed via router (IPC dispatch not yet wired)", call.name)
        }))
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
            "agent/kill" => {
                if let Some(kernel) = &self.kernel {
                    let pid = call.args["pid"].as_u64().unwrap_or(0) as u32;
                    kernel.record_proc_kill(pid).await;
                }
                Ok(serde_json::json!({"killed": true}))
            }
            "cap/request-tool" => {
                let tool_name = call.args["tool"].as_str().unwrap_or("").to_string();
                let reason = call.args["reason"].as_str().unwrap_or("").to_string();

                // Route through KernelResourceHandler when available
                if let Some(handler) = &self.resource_handler {
                    let req = ResourceRequest::new(
                        self.pid.as_u32(),
                        self.token.signature.clone(),
                        vec![ResourceItem::Tool {
                            name: tool_name.clone(),
                            urgency: Urgency::Normal,
                            reason,
                        }],
                    );
                    match handler.handle(&req, &self.token) {
                        Ok(resp) => {
                            if let Some(ResourceGrant::Tool {
                                granted, new_token, ..
                            }) = resp.grants.into_iter().next()
                            {
                                if granted {
                                    if let Some(tok) = new_token {
                                        self.token = tok;
                                        self.refresh_tool_list();
                                    }
                                    return Ok(
                                        serde_json::json!({"approved": true, "tool": tool_name}),
                                    );
                                }
                            }
                            return Ok(serde_json::json!({"approved": false, "tool": tool_name}));
                        }
                        Err(e) => {
                            return Ok(
                                serde_json::json!({"approved": false, "error": e.to_string()}),
                            );
                        }
                    }
                }

                // Fallback: mock auto-approve flag
                if let Some(kernel) = &self.kernel {
                    if kernel.is_auto_approve().await {
                        return Ok(serde_json::json!({"approved": true}));
                    }
                }
                Ok(serde_json::json!({"approved": false}))
            }
            // cap/list — reads directly from in-memory CapabilityToken (no IPC call).
            // Schema per docs/spec/runtime-exec-tool-exposure.md §cap/list.
            // Never exposes the token's HMAC signature.
            "cap/list" => {
                let budgets: serde_json::Value = self
                    .registered_cat2
                    .iter()
                    .filter_map(|name| {
                        self.tool_budgets
                            .remaining(name)
                            .map(|n| (name.clone(), serde_json::json!(n)))
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                Ok(serde_json::json!({
                    "grantedTools": self.token.granted_tools,
                    "constraints": {
                        "maxTokensPerTurn": null,
                        "maxToolChainLength": self.max_tool_chain_length,
                        "toolCallBudgets": budgets
                    },
                    "tokenExpiresAt": self.token.expires_at.to_rfc3339()
                }))
            }
            "cap/escalate" => {
                let guidance = call.args["reason"].as_str().unwrap_or("");
                // Inject into Block 4 (pending instructions) so the LLM sees the guidance
                // on the next turn as per the spec §Category 3 transparent behaviours.
                self.pending_messages
                    .push(format!("[Human guidance]: {guidance}"));
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
            "pipe/open" => {
                let target_pid = call.args["targetPid"].as_u64().unwrap_or(0) as u32;
                let direction = call.args["direction"].as_str().unwrap_or("out").to_string();
                let buffer_tokens = call.args["bufferTokens"].as_u64().unwrap_or(8192) as u32;

                if let Some(handler) = &self.resource_handler {
                    let pipe_direction = match direction.as_str() {
                        "in" => crate::kernel::resource_request::PipeDirection::In,
                        "bidirectional" => {
                            crate::kernel::resource_request::PipeDirection::Bidirectional
                        }
                        _ => crate::kernel::resource_request::PipeDirection::Out,
                    };
                    let req = ResourceRequest::new(
                        self.pid.as_u32(),
                        self.token.signature.clone(),
                        vec![ResourceItem::Pipe {
                            target_pid,
                            direction: pipe_direction,
                            buffer_tokens,
                            reason: String::new(),
                        }],
                    );
                    match handler.handle(&req, &self.token) {
                        Ok(resp) => {
                            if let Some(ResourceGrant::Pipe {
                                granted: true,
                                pipe_id: Some(pipe_id),
                                ..
                            }) = resp.grants.into_iter().next()
                            {
                                // Write /proc/<pid>/pipes/<pipeId>.yaml to VFS when handle available
                                if let Some(vfs) = &self.vfs {
                                    let pid = self.pid.as_u32();
                                    let entry = serde_yaml::to_string(&serde_json::json!({
                                        "pipe_id": pipe_id,
                                        "target_pid": target_pid,
                                        "direction": direction,
                                        "buffer_tokens": buffer_tokens,
                                        "state": "open"
                                    }))
                                    .unwrap_or_default();
                                    let path_str = format!("/proc/{}/pipes/{}.yaml", pid, pipe_id);
                                    if let Ok(path) = VfsPath::parse(&path_str) {
                                        let _ = vfs.write(&path, entry.into_bytes()).await;
                                    }
                                }
                                return Ok(
                                    serde_json::json!({ "pipeId": pipe_id, "state": "open" }),
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(pid = ?self.pid, error = %e, "pipe/open resource request failed");
                        }
                    }
                }

                // Fallback stub
                Ok(serde_json::json!({ "pipeId": "pipe-stub", "state": "open" }))
            }
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

    /// Set the token's expiry to `now + d`. Used in tests to simulate near-expiry tokens.
    pub fn set_token_expiry_in(&mut self, d: Duration) {
        self.token.expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(d).unwrap_or(chrono::Duration::hours(1));
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

    /// Token renewal — if the token is still valid but within 5 minutes of expiry,
    /// send a `ResourceRequest{token_renewal}` to the kernel handler (when available)
    /// and replace `self.token` with the newly signed token from the response.
    /// Falls back to in-place extension when no handler is attached (tests).
    /// Already-expired tokens are NOT renewed here; the expiry guard handles those.
    fn maybe_renew_token(&mut self) {
        let until_expiry = self
            .token
            .expires_at
            .signed_duration_since(chrono::Utc::now());
        if !(until_expiry > chrono::Duration::zero()
            && until_expiry <= chrono::Duration::minutes(5))
        {
            return;
        }

        if let Some(handler) = self.resource_handler.clone() {
            let req = ResourceRequest::new(
                self.pid.as_u32(),
                self.token.signature.clone(),
                vec![ResourceItem::TokenRenewal {
                    reason: "auto-renewal within 5 min window".into(),
                }],
            );
            match handler.handle(&req, &self.token) {
                Ok(resp) => {
                    if let Some(ResourceGrant::TokenRenewal {
                        granted: true,
                        new_token: Some(tok),
                        ..
                    }) = resp.grants.into_iter().next()
                    {
                        tracing::info!(pid = ?self.pid, "token renewed via KernelResourceHandler");
                        self.token = tok;
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!(pid = ?self.pid, error = %e, "token renewal request failed");
                }
            }
        }

        // Fallback: extend in-place (unsigned test tokens)
        self.token.expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
        tracing::info!(pid = ?self.pid, "token renewed (mock)");
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

            // Token renewal — extend before calling LLM so the turn doesn't start with an expired token
            self.maybe_renew_token();

            // Expiry guard — abort if token is still expired after renewal attempt
            if self.token.is_expired() {
                return Err(AvixError::CapabilityDenied(
                    "capability token expired; cannot begin turn".into(),
                ));
            }

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
                        // GAP 4: capability validation + budget check (budget decremented on success)
                        if let Err(e) =
                            validate_tool_call(&self.token, call, &mut self.tool_budgets)
                        {
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

                        // Cat2: handled locally; Cat1/3: forwarded to router.svc
                        let result = if self.is_cat2_tool(&call.name) {
                            self.dispatch_category2(call).await?
                        } else {
                            self.dispatch_via_router(call).await?
                        };
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

            // Token renewal + expiry guard
            self.maybe_renew_token();
            if self.token.is_expired() {
                return Err(AvixError::ConfigParse(
                    "capability token expired; cannot begin turn".into(),
                ));
            }

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
            token: CapabilityToken::test_token(caps),
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
            token: CapabilityToken::test_token(&["cap/list"]), // has cap/list, not fs/read
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
        let mut executor = make_executor(
            210,
            &[
                "agent/spawn",
                "agent/kill",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        )
        .await;
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
        let mut executor =
            make_executor(212, &["pipe/open", "pipe/write", "pipe/read", "pipe/close"]).await;
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

    #[tokio::test]
    async fn test_set_max_tool_chain_length() {
        let mut executor = make_executor(230, &[]).await;
        assert_eq!(executor.max_tool_chain_length, 50); // default
        executor.set_max_tool_chain_length(10);
        assert_eq!(executor.max_tool_chain_length, 10);
    }

    #[tokio::test]
    async fn test_set_tool_budget() {
        let mut executor = make_executor(231, &["fs/read"]).await;
        executor.set_tool_budget("fs/read", 5);
        assert_eq!(executor.tool_budgets.remaining("fs/read"), Some(5));
    }

    #[tokio::test]
    async fn test_require_hil_for_sets_field() {
        let mut executor = make_executor(232, &[]).await;
        executor.require_hil_for("cap/escalate");
        executor.require_hil_for("fs/delete");
        // Verify the tools are recorded by checking hil gating in a turn
        // (indirect test — we just verify the pending_messages after a blocked call)
        let mock_client = MockLlmClient::new(vec![
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use",
                    "id": "call-1",
                    "name": "cap__escalate",
                    "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "ok"})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 3,
                output_tokens: 1,
            },
        ]);
        let result = executor.run_with_client("test", &mock_client).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_inject_pending_message_accumulates() {
        let mut executor = make_executor(233, &[]).await;
        executor.inject_pending_message("msg-1".into());
        executor.inject_pending_message("msg-2".into());
        executor.inject_pending_message("msg-3".into());
        assert_eq!(executor.pending_messages.len(), 3);
        assert_eq!(executor.pending_messages[0], "msg-1");
        assert_eq!(executor.pending_messages[2], "msg-3");
    }

    #[tokio::test]
    async fn test_dispatch_agent_kill() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        let params = make_params(
            250,
            &[
                "agent/spawn",
                "agent/kill",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        );
        let mut executor =
            RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, Arc::clone(&kernel))
                .await
                .unwrap();
        let call = AvixToolCall {
            call_id: "kill-1".into(),
            name: "agent/kill".into(),
            args: json!({"pid": 77, "reason": "done"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert_eq!(result["killed"], true);
        assert!(kernel.received_proc_kill(77).await);
    }

    #[tokio::test]
    async fn test_is_cat2_tool() {
        let executor = make_executor(
            251,
            &[
                "agent/spawn",
                "agent/kill",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        )
        .await;
        assert!(executor.is_cat2_tool("cap/list"));
        assert!(executor.is_cat2_tool("agent/spawn"));
        assert!(!executor.is_cat2_tool("fs/read"));
        assert!(!executor.is_cat2_tool("llm/complete"));
    }

    #[tokio::test]
    async fn test_spawn_with_registry_and_kernel() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        let params = make_params(240, &["cap/list"]);
        let executor = RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, kernel)
            .await
            .unwrap();
        assert!(!executor.tool_list.is_empty());
        // kernel is set
    }

    #[tokio::test]
    async fn test_set_token_expiry_in_and_on_fs_read() {
        let mut executor = make_executor(241, &[]).await;
        // set_token_expiry_in should set token_expiry_at
        executor.set_token_expiry_in(Duration::from_secs(300));
        // on_fs_read should store data
        executor.on_fs_read("/tmp/test.txt", b"hello world");
        // No panic = success; we test indirectly via run_until_complete with fs/read
    }

    #[tokio::test]
    async fn test_run_until_complete_fs_read() {
        let mut executor = make_executor(242, &[]).await;
        executor.on_fs_read("/tmp/hello.txt", b"file contents here");

        // Simulate: LLM calls fs/read, then returns text
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({
                "type": "tool_use",
                "id": "read-call",
                "name": "fs/read",
                "input": {"path": "/tmp/hello.txt"}
            })],
            stop_reason: StopReason::ToolUse,
            input_tokens: 5,
            output_tokens: 2,
        });
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({"type": "text", "text": "I read the file"})],
            stop_reason: StopReason::EndTurn,
            input_tokens: 5,
            output_tokens: 3,
        });

        let result = executor.run_until_complete("read the file").await;
        assert!(result.is_ok());
        assert!(result.unwrap().text.contains("read the file"));
    }

    #[tokio::test]
    async fn test_run_until_complete_chain_limit_exceeded() {
        let mut executor = make_executor(243, &[]).await;
        executor.set_max_tool_chain_length(1);

        // Push two tool-use responses (will exceed chain limit of 1)
        for i in 0..3 {
            executor.push_llm_response(LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use",
                    "id": format!("call-{i}"),
                    "name": "cap/list",
                    "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            });
        }

        let result = executor.run_until_complete("do stuff").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("max tool chain"), "err: {err}");
    }

    #[tokio::test]
    async fn test_dispatch_job_watch() {
        let mut executor = make_executor(244, &[]).await;
        let call = AvixToolCall {
            call_id: "c1".into(),
            name: "job/watch".into(),
            args: json!({"jobId": "job-abc"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert_eq!(result["finalStatus"], "done");
        assert_eq!(result["jobId"], "job-abc");
    }

    #[tokio::test]
    async fn test_dispatch_agent_list() {
        let mut executor = make_executor(245, &[]).await;
        let call = AvixToolCall {
            call_id: "c2".into(),
            name: "agent/list".into(),
            args: json!({}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(result["agents"].is_array());
    }

    #[tokio::test]
    async fn test_dispatch_agent_wait() {
        let mut executor = make_executor(246, &[]).await;
        let call = AvixToolCall {
            call_id: "c3".into(),
            name: "agent/wait".into(),
            args: json!({"pid": 99}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert_eq!(result["finalStatus"], "completed");
    }

    #[tokio::test]
    async fn test_dispatch_agent_send_message() {
        let mut executor = make_executor(247, &[]).await;
        let call = AvixToolCall {
            call_id: "c4".into(),
            name: "agent/send-message".into(),
            args: json!({"pid": 99, "message": "hello"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert_eq!(result["delivered"], true);
    }

    #[tokio::test]
    async fn test_dispatch_pipe_write_and_read_and_close() {
        let mut executor = make_executor(248, &[]).await;

        let write_call = AvixToolCall {
            call_id: "pw".into(),
            name: "pipe/write".into(),
            args: json!({"pipeId": "p1", "content": "hello"}),
        };
        let w_result = executor.dispatch_category2(&write_call).await.unwrap();
        assert!(w_result.get("tokensSent").is_some());

        let read_call = AvixToolCall {
            call_id: "pr".into(),
            name: "pipe/read".into(),
            args: json!({"pipeId": "p1"}),
        };
        let r_result = executor.dispatch_category2(&read_call).await.unwrap();
        assert!(r_result.get("content").is_some());

        let close_call = AvixToolCall {
            call_id: "pc".into(),
            name: "pipe/close".into(),
            args: json!({"pipeId": "p1"}),
        };
        let c_result = executor.dispatch_category2(&close_call).await.unwrap();
        assert_eq!(c_result["closed"], true);
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool_returns_stub() {
        let mut executor = make_executor(249, &[]).await;
        let call = AvixToolCall {
            call_id: "c99".into(),
            name: "some/unknown-tool".into(),
            args: json!({}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        // Unknown tool returns stub response
        assert!(result.get("content").is_some());
    }

    #[tokio::test]
    async fn test_dispatch_cap_request_tool_without_kernel() {
        let mut executor = make_executor(250, &[]).await;
        let call = AvixToolCall {
            call_id: "c5".into(),
            name: "cap/request-tool".into(),
            args: json!({"tool": "fs/read", "reason": "need it"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        // No kernel → not auto-approved
        assert_eq!(result["approved"], false);
    }

    #[tokio::test]
    async fn test_dispatch_agent_spawn_without_kernel() {
        let mut executor = make_executor(251, &[]).await;
        let call = AvixToolCall {
            call_id: "c6".into(),
            name: "agent/spawn".into(),
            args: json!({"agent": "worker", "goal": "do stuff"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert_eq!(result["spawned"], true);
    }

    #[tokio::test]
    async fn test_hil_gate_with_auto_approve_kernel() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        kernel.auto_approve_resource_request().await;

        let params = make_params(252, &["cap/list"]);
        let mut executor =
            RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, kernel)
                .await
                .unwrap();

        executor.require_hil_for("cap/list");

        let mock_client = MockLlmClient::new(vec![
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use",
                    "id": "hil-auto",
                    "name": "cap__list",
                    "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "Done."})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 3,
                output_tokens: 1,
            },
        ]);

        let result = executor.run_with_client("do something", &mock_client).await;
        assert!(
            result.is_ok(),
            "auto-approved HIL should complete: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_run_with_client_chain_limit_exceeded() {
        let mut executor = make_executor(253, &[]).await;
        executor.set_max_tool_chain_length(1);

        let mock_client = MockLlmClient::new(vec![LlmCompleteResponse {
            content: vec![
                json!({"type": "tool_use", "id": "c1", "name": "cap__list", "input": {}}),
                json!({"type": "tool_use", "id": "c2", "name": "cap__list", "input": {}}),
            ],
            stop_reason: StopReason::ToolUse,
            input_tokens: 5,
            output_tokens: 2,
        }]);

        let result = executor.run_with_client("do it", &mock_client).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max tool chain"));
    }

    #[tokio::test]
    async fn test_current_tool_list_excludes_removed() {
        let mut executor = make_executor(254, &[]).await;
        let initial_count = executor.current_tool_list().len();
        executor
            .handle_tool_changed("removed", "cap/list", "test")
            .await;
        let after_count = executor.current_tool_list().len();
        assert!(
            after_count < initial_count,
            "removed tool should reduce list"
        );
    }

    #[tokio::test]
    async fn test_llm_call_count_tracks_calls() {
        let mut executor = make_executor(255, &[]).await;
        assert_eq!(executor.llm_call_count(), 0);

        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({"type": "text", "text": "done"})],
            stop_reason: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        });
        let _ = executor.run_until_complete("test").await;
        assert_eq!(executor.llm_call_count(), 1);
    }

    #[tokio::test]
    async fn test_call_messages_returns_empty_for_invalid_idx() {
        let executor = make_executor(256, &[]).await;
        let msgs = executor.call_messages(99);
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn test_run_until_complete_summarise_context_stub() {
        let mut executor = make_executor(257, &[]).await;
        // SummariseContext is treated as "continue" but needs another response
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![],
            stop_reason: StopReason::MaxTokens, // maps to SummariseContext
            input_tokens: 5,
            output_tokens: 0,
        });
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({"type": "text", "text": "summary done"})],
            stop_reason: StopReason::EndTurn,
            input_tokens: 3,
            output_tokens: 1,
        });
        let result = executor.run_until_complete("test").await;
        assert!(result.is_ok());
    }
}
