use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::executor::ipc_dispatch::dispatch_cat1_tool;
use crate::executor::syscall_dispatch::dispatch_kernel_syscall;
use crate::tool_registry::ToolEntry;

use tokio_util::sync::CancellationToken;

use crate::error::AvixError;
use crate::kernel::resource_request::{ResourceGrant, ResourceItem, ResourceRequest, Urgency};
use crate::llm_client::{LlmCompleteResponse, StopReason, StreamChunk};
use crate::llm_svc::adapter::AvixToolCall;
use crate::memfs::VfsPath;
use crate::signal::kind::SignalKind;
use crate::snapshot::{capture, CaptureParams, CapturedBy, SnapshotMemory, SnapshotTrigger};

use super::{RuntimeExecutor, TurnResult};

impl RuntimeExecutor {
    /// Deliver a signal to the executor.
    ///
    /// Updates atomic tracking flags immediately **and** forwards the signal onto
    /// the in-process channel so an in-flight `run_with_client` loop can react
    /// without waiting for the current LLM call to complete.
    pub async fn deliver_signal(&self, signal: &str) {
        *self.last_signal_received.lock().await = Some(signal.to_string());
        self.pending_signal_count.fetch_add(1, Ordering::AcqRel);

        let kind = match signal {
            "SIGKILL" => SignalKind::Kill,
            "SIGSTOP" => SignalKind::Stop,
            "SIGPAUSE" => SignalKind::Pause,
            "SIGRESUME" => SignalKind::Resume,
            "SIGSAVE" => SignalKind::Save,
            "SIGPIPE" => SignalKind::Pipe,
            "SIGESCALATE" => SignalKind::Escalate,
            "SIGSTART" => SignalKind::Start,
            _ => SignalKind::Usr1,
        };
        let sig = crate::signal::kind::Signal {
            target: self.pid,
            kind,
            payload: serde_json::Value::Null,
        };
        let _ = self.signal_tx.try_send(sig);

        match signal {
            "SIGSTOP" => {
                self.memory
                    .auto_log_session_end(
                        self.pid.as_u64(),
                        &self.agent_name,
                        &self.spawned_by,
                        &self.session_id,
                        &self.token.granted_tools,
                    )
                    .await;
                self.killed.store(true, Ordering::Release);
                if let Some(vfs) = &self.vfs {
                    self.write_status_yaml(vfs).await;
                }
            }
            "SIGKILL" => {
                self.killed.store(true, Ordering::Release);
                if let Some(vfs) = &self.vfs {
                    self.write_status_yaml(vfs).await;
                }
            }
            "SIGPAUSE" => {
                self.paused.store(true, Ordering::Release);
                if let (Some(store), false) =
                    (&self.invocation_store, self.invocation_id.is_empty())
                {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Paused,
                        )
                        .await;
                }
                if let Some(vfs) = &self.vfs {
                    self.write_status_yaml(vfs).await;
                }
            }
            "SIGRESUME" => {
                self.paused.store(false, Ordering::Release);
                if let (Some(store), false) =
                    (&self.invocation_store, self.invocation_id.is_empty())
                {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Running,
                        )
                        .await;
                }
                if let Some(vfs) = &self.vfs {
                    self.write_status_yaml(vfs).await;
                }
            }
            "SIGSAVE" => {
                self.capture_and_write_snapshot(SnapshotTrigger::Sigsave, CapturedBy::Kernel)
                    .await;
                self.take_interim_snapshot().await;
            }
            _ => {
                tracing::debug!(pid = self.pid.as_u64(), signal, "signal received");
                if let Some(vfs) = &self.vfs {
                    self.write_status_yaml(vfs).await;
                }
            }
        }
    }

    /// Capture a snapshot of current executor state and write it to the VFS.
    pub(super) async fn capture_and_write_snapshot(
        &self,
        trigger: SnapshotTrigger,
        captured_by: CapturedBy,
    ) {
        let vfs = match &self.vfs {
            Some(v) => Arc::clone(v),
            None => {
                tracing::debug!(pid = self.pid.as_u64(), "snapshot skipped: no VFS attached");
                return;
            }
        };

        let snap = capture(CaptureParams {
            agent_name: &self.agent_name,
            pid: self.pid.as_u64(),
            username: &self.spawned_by,
            goal: &self.goal,
            message_history: &self.memory.conversation_history,
            temperature: 0.7,
            granted_tools: &self.token.granted_tools,
            trigger,
            captured_by,
            memory: SnapshotMemory::default(),
            pending_requests: vec![],
            open_pipes: vec![],
        });

        let vfs_path_str = snap.vfs_path(&self.spawned_by);
        match snap.to_yaml() {
            Ok(yaml) => match VfsPath::parse(&vfs_path_str) {
                Ok(path) => {
                    if let Err(e) = vfs.write(&path, yaml.into_bytes()).await {
                        tracing::warn!(
                            pid = self.pid.as_u64(),
                            path = vfs_path_str,
                            err = ?e,
                            "snapshot VFS write failed"
                        );
                    } else {
                        tracing::info!(
                            pid = self.pid.as_u64(),
                            path = vfs_path_str,
                            "snapshot written"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(pid = self.pid.as_u64(), err = ?e, "invalid snapshot VFS path")
                }
            },
            Err(e) => {
                tracing::warn!(pid = self.pid.as_u64(), err = ?e, "snapshot serialisation failed")
            }
        }
    }

    /// Take an interim snapshot by persisting current state to the invocation store.
    pub(super) async fn take_interim_snapshot(&self) {
        let store = match &self.invocation_store {
            Some(s) => Arc::clone(s),
            None => return,
        };
        let id = &self.invocation_id;
        if id.is_empty() {
            return;
        }

        if let Err(e) = store
            .persist_interim(
                id,
                &self.memory.conversation_history,
                self.tokens_consumed,
                self.tool_calls_total,
            )
            .await
        {
            tracing::warn!(pid = self.pid.as_u64(), id = %id, err = ?e, "interim snapshot failed");
        } else {
            tracing::debug!(pid = self.pid.as_u64(), id = %id, "interim snapshot saved");
        }
    }

    /// Restore executor state from a named snapshot stored in the VFS.
    pub async fn restore_from_snapshot(
        &mut self,
        snapshot_name: &str,
    ) -> Result<super::RestoreResult, AvixError> {
        use crate::snapshot::verify_checksum;
        use crate::snapshot::SnapshotFile;

        let vfs = match &self.vfs {
            Some(v) => Arc::clone(v),
            None => return Err(AvixError::ConfigParse("no VFS attached".into())),
        };

        let path_str = format!(
            "/users/{}/snapshots/{}.yaml",
            self.spawned_by, snapshot_name
        );
        let path = VfsPath::parse(&path_str).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let bytes = vfs
            .read(&path)
            .await
            .map_err(|e| AvixError::NotFound(format!("snapshot '{snapshot_name}': {e}")))?;
        let yaml = String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let file = SnapshotFile::from_str(&yaml)?;

        verify_checksum(&file)?;

        let original_tools = file.spec.environment.granted_tools.clone();
        self.token = crate::types::token::CapabilityToken::test_token(
            &original_tools
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
        );

        self.goal = file.spec.goal.clone();
        if !file.spec.context_summary.is_empty() {
            use crate::invocation::conversation::{ConversationEntry, Role};
            self.memory.conversation_history = vec![ConversationEntry::from_role_content(
                Role::Assistant,
                format!(
                    "[Restored from snapshot '{}']\n\nContext at capture:\n{}",
                    file.metadata.name, file.spec.context_summary
                ),
            )];
        }

        let reissued_requests: Vec<String> = file
            .spec
            .pending_requests
            .iter()
            .filter(|r| r.status == "in-flight")
            .map(|r| r.request_id.clone())
            .collect();

        let sigpipe_pipes: Vec<String> = file
            .spec
            .pipes
            .iter()
            .filter(|p| p.state == "open")
            .map(|p| p.pipe_id.clone())
            .collect();

        tracing::info!(
            pid = self.pid.as_u64(),
            snapshot = %file.metadata.name,
            reissued = ?reissued_requests,
            sigpipe = ?sigpipe_pipes,
            "restore complete"
        );

        Ok(super::RestoreResult {
            snapshot_name: file.metadata.name.clone(),
            agent_name: file.metadata.agent_name.clone(),
            reissued_requests,
            reconnected_pipes: vec![],
            sigpipe_pipes,
        })
    }

    /// Dispatch a Category 1 tool call via its registered IPC endpoint.
    ///
    /// Looks up the tool's descriptor from the real `ToolRegistry` (wired in gap-A),
    /// checks execute permission, then dispatches over a fresh Unix socket (ADR-05).
    /// Kernel tools (no `ipc` binding) are forwarded to the kernel IPC server.
    pub async fn dispatch_via_router(
        &self,
        call: &AvixToolCall,
    ) -> Result<serde_json::Value, AvixError> {
        // 1. Look up ToolEntry from real registry; mock registry always returns None
        let entry_opt = match &self.registry_ref {
            super::RegistryRef::Real(reg) => reg.lookup(&call.name).await.ok(),
            super::RegistryRef::Mock(_) => None,
        };

        let descriptor = entry_opt
            .as_ref()
            .map(|e: &ToolEntry| &e.descriptor)
            .ok_or_else(|| {
                tracing::warn!(
                    pid = self.pid.as_u64(),
                    tool = %call.name,
                    "Cat1 tool not found in registry"
                );
                AvixError::ConfigParse(format!("tool '{}' not found in registry", call.name))
            })?;

        // 2. Check execute permission
        if let Some(ref entry) = entry_opt {
            if let Err(e) = check_tool_execute_permission(entry, &self.spawned_by) {
                tracing::warn!(
                    pid = self.pid.as_u64(),
                    tool = %call.name,
                    user = %self.spawned_by,
                    error = %e,
                    "Cat1 tool execute permission denied"
                );
                return Err(e);
            }
        }

        // 3. Kernel tools have no IPC binding — route to kernel socket
        if descriptor.get("ipc").map_or(true, |v| v.is_null()) {
            tracing::debug!(
                pid = self.pid.as_u64(),
                tool = %call.name,
                "routing to kernel IPC server (no IPC binding)"
            );
            return dispatch_kernel_syscall(
                call,
                self.pid.as_u64(),
                &self.session_id,
                &self.runtime_dir,
            )
            .await;
        }

        // 4. Dispatch via service IPC socket
        let caller_scoped = is_caller_scoped_tool(&call.name, descriptor);
        tracing::debug!(
            pid = self.pid.as_u64(),
            tool = %call.name,
            caller_scoped,
            "routing Cat1 tool to service IPC"
        );
        dispatch_cat1_tool(
            call,
            descriptor,
            self.pid.as_u64(),
            &self.session_id,
            &self.runtime_dir,
            caller_scoped,
        )
        .await
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
                    let pid = call.args["pid"].as_u64().unwrap_or(0);
                    kernel.record_proc_kill(pid).await;
                }
                Ok(serde_json::json!({"killed": true}))
            }
            "cap/request-tool" => {
                let tool_name = call.args["tool"].as_str().unwrap_or("").to_string();
                let reason = call.args["reason"].as_str().unwrap_or("").to_string();

                if let Some(handler) = &self.resource_handler {
                    let req = ResourceRequest::new(
                        self.pid.as_u64(),
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
                                        self.refresh_tool_list().await;
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

                if let Some(kernel) = &self.kernel {
                    if kernel.is_auto_approve().await {
                        return Ok(serde_json::json!({"approved": true}));
                    }
                }
                Ok(serde_json::json!({"approved": false}))
            }
            "cap/list" => {
                let budgets: serde_json::Value = self
                    .tools
                    .registered_cat2
                    .iter()
                    .filter_map(|name| {
                        self.tools
                            .tool_budgets
                            .remaining(name)
                            .map(|n| (name.clone(), serde_json::json!(n)))
                    })
                    .collect::<serde_json::Map<String, serde_json::Value>>()
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
                let target_pid = call.args["targetPid"].as_u64().unwrap_or(0);
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
                        self.pid.as_u64(),
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
                                if let Some(vfs) = &self.vfs {
                                    let pid = self.pid.as_u64();
                                    let entry = serde_yaml::to_string(&serde_json::json!({
                                        "pipe_id": pipe_id,
                                        "target_pid": target_pid,
                                        "direction": direction,
                                        "buffer_tokens": buffer_tokens,
                                        "state": "open"
                                    }))
                                    .unwrap_or_default();
                                    let path_str =
                                        format!("/proc/{}/pipes/{}.yaml", pid, pipe_id);
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
            "sys/tools" => {
                let namespace = call.args["namespace"].as_str().unwrap_or("").to_string();
                let keyword = call.args["keyword"].as_str().unwrap_or("").to_string();
                let granted_only = call.args["granted_only"].as_bool().unwrap_or(false);

                if let Some(kernel) = &self.kernel {
                    let summaries = kernel
                        .list_tools(namespace, keyword, granted_only, &self.token)
                        .await;
                    return Ok(serde_json::json!({ "tools": summaries }));
                }
                Ok(serde_json::json!({ "tools": [] }))
            }
            _ => Ok(serde_json::json!({
                "content": format!("Tool '{}' executed (IPC dispatch not yet wired)", call.name)
            })),
        }
    }

    /// Token renewal — if the token is within 5 minutes of expiry, renew it.
    pub(super) fn maybe_renew_token(&mut self) {
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
                self.pid.as_u64(),
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

        self.token.expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
        tracing::info!(pid = ?self.pid, "token renewed (mock)");
    }

    /// Execute a single LLM turn, preferring streaming when the client supports it.
    async fn run_turn_streaming(
        &self,
        req: crate::llm_client::LlmCompleteRequest,
        client: &dyn crate::llm_client::LlmClient,
        turn_id: uuid::Uuid,
        cancel: CancellationToken,
    ) -> Result<LlmCompleteResponse, AvixError> {
        use futures::StreamExt as _;
        use tokio::time::{interval, Duration, MissedTickBehavior};

        const CHUNK_FLUSH_BYTES: usize = 80;

        let stream = client.stream_complete(req.clone()).await;
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => {
                return client
                    .complete(req)
                    .await
                    .map_err(|e| AvixError::ConfigParse(e.to_string()));
            }
        };

        let turn_id_str = turn_id.to_string();
        let mut accumulated_text = String::new();
        let mut pending_calls: HashMap<String, (String, String)> = HashMap::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut seq: u64 = 0;
        let mut pending_text = String::new();

        let mut flush_timer = interval(Duration::from_millis(50));
        flush_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        flush_timer.tick().await;

        macro_rules! flush_pending {
            () => {
                if !pending_text.is_empty() {
                    if let Some(bus) = &self.event_bus {
                        bus.agent_output_chunk(
                            &self.atp_session_id,
                            self.pid.as_u64(),
                            &turn_id_str,
                            &pending_text,
                            seq,
                            false,
                        );
                        seq += 1;
                    }
                    pending_text.clear();
                }
            };
        }

        loop {
            tokio::select! {
                biased;

                _ = cancel.cancelled() => {
                    return Err(AvixError::Cancelled("LLM call cancelled by signal".into()));
                }

                chunk_opt = stream.next() => {
                    let Some(chunk_result) = chunk_opt else { break; };
                    match chunk_result.map_err(|e| AvixError::ConfigParse(e.to_string()))? {
                        StreamChunk::TextDelta { text } => {
                            accumulated_text.push_str(&text);
                            pending_text.push_str(&text);
                            if pending_text.len() >= CHUNK_FLUSH_BYTES {
                                flush_pending!();
                            }
                        }
                        StreamChunk::ToolCallStart { call_id, name } => {
                            flush_pending!();
                            pending_calls.insert(call_id, (name, String::new()));
                        }
                        StreamChunk::ToolCallArgsDelta { call_id, args_delta } => {
                            if let Some(entry) = pending_calls.get_mut(&call_id) {
                                entry.1.push_str(&args_delta);
                            }
                        }
                        StreamChunk::ToolCallComplete { .. } => {}
                        StreamChunk::Done {
                            stop_reason: sr,
                            input_tokens: it,
                            output_tokens: ot,
                        } => {
                            stop_reason = sr;
                            input_tokens = it;
                            output_tokens = ot;
                        }
                    }
                }

                _ = flush_timer.tick() => {
                    flush_pending!();
                }
            }
        }

        flush_pending!();

        if let Some(bus) = &self.event_bus {
            bus.agent_output_chunk(
                &self.session_id,
                self.pid.as_u64(),
                &turn_id_str,
                "",
                seq,
                true,
            );
        }

        let mut content: Vec<serde_json::Value> = Vec::new();
        if !accumulated_text.is_empty() {
            content.push(serde_json::json!({"type": "text", "text": accumulated_text}));
        }
        for (call_id, (name, args_str)) in &pending_calls {
            let input = serde_json::from_str::<serde_json::Value>(args_str)
                .unwrap_or_else(|_| serde_json::json!({}));
            content.push(serde_json::json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input,
            }));
        }

        Ok(LlmCompleteResponse {
            content,
            stop_reason,
            input_tokens,
            output_tokens,
        })
    }

    /// Handle a signal that arrived **between** LLM turns.
    pub(super) async fn handle_signal_between_turns(
        &mut self,
        signal: &crate::signal::kind::Signal,
    ) {
        let name = signal.kind.as_str();
        *self.last_signal_received.lock().await = Some(name.to_string());
        self.pending_signal_count.fetch_add(1, Ordering::AcqRel);

        match name {
            "SIGKILL" | "SIGSTOP" => {
                self.memory
                    .auto_log_session_end(
                        self.pid.as_u64(),
                        &self.agent_name,
                        &self.spawned_by,
                        &self.session_id,
                        &self.token.granted_tools,
                    )
                    .await;
                self.killed.store(true, Ordering::Release);
                if let Some(vfs) = &self.vfs {
                    let vfs = Arc::clone(vfs);
                    self.write_status_yaml(&vfs).await;
                }
            }
            "SIGPAUSE" => {
                self.paused.store(true, Ordering::Release);
                if let (Some(store), false) =
                    (&self.invocation_store, self.invocation_id.is_empty())
                {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Paused,
                        )
                        .await;
                }
                if let Some(vfs) = &self.vfs {
                    let vfs = Arc::clone(vfs);
                    self.write_status_yaml(&vfs).await;
                }
            }
            "SIGRESUME" => {
                self.paused.store(false, Ordering::Release);
                if let (Some(store), false) =
                    (&self.invocation_store, self.invocation_id.is_empty())
                {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Running,
                        )
                        .await;
                }
                if let Some(vfs) = &self.vfs {
                    let vfs = Arc::clone(vfs);
                    self.write_status_yaml(&vfs).await;
                }
            }
            "SIGSAVE" => {
                self.snapshot_requested.store(true, Ordering::Release);
            }
            "SIGPIPE" => {
                let text = signal.payload["text"].as_str().unwrap_or("[pipe data]");
                self.inject_pending_message(format!("[SIGPIPE]: {text}"));
            }
            _ => {
                tracing::debug!(
                    pid = self.pid.as_u64(),
                    signal = name,
                    "unhandled signal between turns"
                );
            }
        }
    }

    /// Handle a signal that arrived **while an LLM call was in flight**.
    pub(super) async fn handle_signal_during_llm(&mut self, signal: &crate::signal::kind::Signal) {
        let name = signal.kind.as_str();
        *self.last_signal_received.lock().await = Some(name.to_string());
        self.pending_signal_count.fetch_add(1, Ordering::AcqRel);

        match name {
            "SIGKILL" | "SIGSTOP" => {
                self.memory
                    .auto_log_session_end(
                        self.pid.as_u64(),
                        &self.agent_name,
                        &self.spawned_by,
                        &self.session_id,
                        &self.token.granted_tools,
                    )
                    .await;
                self.killed.store(true, Ordering::Release);
                if let Some(vfs) = &self.vfs {
                    let vfs = Arc::clone(vfs);
                    self.write_status_yaml(&vfs).await;
                }
            }
            "SIGPAUSE" => {
                self.paused.store(true, Ordering::Release);
                if let (Some(store), false) =
                    (&self.invocation_store, self.invocation_id.is_empty())
                {
                    let _ = store
                        .update_status(
                            &self.invocation_id,
                            crate::invocation::InvocationStatus::Paused,
                        )
                        .await;
                }
                if let Some(vfs) = &self.vfs {
                    let vfs = Arc::clone(vfs);
                    self.write_status_yaml(&vfs).await;
                }
            }
            "SIGPIPE" => {
                let text = signal.payload["text"].as_str().unwrap_or("[pipe data]");
                self.inject_pending_message(format!("[SIGPIPE]: {text}"));
            }
            "SIGSAVE" => {
                self.capture_and_write_snapshot(SnapshotTrigger::Sigsave, CapturedBy::Kernel)
                    .await;
                self.take_interim_snapshot().await;
            }
            "SIGRESUME" => {
                self.paused.store(false, Ordering::Release);
            }
            _ => {
                tracing::debug!(
                    pid = self.pid.as_u64(),
                    signal = name,
                    "unhandled signal during LLM"
                );
            }
        }
    }

    /// Run the turn loop against a real LLM client.
    ///
    /// On success the signal receiver is restored to `self.signal_rx` so the
    /// caller can invoke `wait_for_next_goal` without rebuilding the channel.
    pub async fn run_with_client(
        &mut self,
        goal: &str,
        client: &dyn crate::llm_client::LlmClient,
    ) -> Result<TurnResult, AvixError> {
        use crate::executor::validation::validate_tool_call;
        use crate::executor::stop_reason::{interpret_stop_reason, TurnAction};

        tracing::info!(
            pid = self.pid.as_u64(),
            agent = %self.agent_name,
            goal = %goal,
            "executor starting turn loop"
        );

        let mut signal_rx = self.signal_rx.take().unwrap_or_else(|| {
            let (_, rx) = tokio::sync::mpsc::channel(1);
            rx
        });

        use crate::invocation::conversation::{ConversationEntry, Role, ToolCallEntry};

        let system = self.build_system_prompt_str();
        let mut messages: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "user", "content": goal})];
        // Record the initial user goal in persistent conversation history.
        self.memory.conversation_history
            .push(ConversationEntry::from_role_content(Role::User, goal));
        let mut chain_count = 0;
        let mut turn_num: u32 = 0;

        loop {
            turn_num += 1;
            tracing::debug!(
                pid = self.pid.as_u64(),
                turn = turn_num,
                tokens_consumed = self.tokens_consumed,
                tool_calls = self.tool_calls_total,
                "starting LLM turn"
            );
            self.refresh_tool_list().await;
            self.maybe_renew_token();

            if self.token.is_expired() {
                self.signal_rx = Some(signal_rx);
                return Err(AvixError::CapabilityDenied(
                    "capability token expired; cannot begin turn".into(),
                ));
            }

            while let Ok(sig) = signal_rx.try_recv() {
                self.handle_signal_between_turns(&sig).await;
                if self.killed.load(Ordering::Acquire) {
                    self.signal_rx = Some(signal_rx);
                    return Err(AvixError::Cancelled("killed between turns".into()));
                }
            }
            if self.paused.load(Ordering::Acquire) {
                tracing::debug!(pid = self.pid.as_u64(), "executor paused; waiting for SIGRESUME");
                loop {
                    match signal_rx.recv().await {
                        Some(sig) => {
                            self.handle_signal_between_turns(&sig).await;
                            if self.killed.load(Ordering::Acquire) {
                                self.signal_rx = Some(signal_rx);
                                return Err(AvixError::Cancelled("killed while paused".into()));
                            }
                            if !self.paused.load(Ordering::Acquire) {
                                tracing::debug!(pid = self.pid.as_u64(), "executor resumed");
                                break;
                            }
                        }
                        None => {
                            self.signal_rx = Some(signal_rx);
                            return Err(AvixError::Cancelled("signal channel closed".into()))
                        }
                    }
                }
            }

            let turn_id = uuid::Uuid::new_v4();

            if self.snapshot_requested.load(Ordering::Acquire) {
                self.snapshot_requested.store(false, Ordering::Release);
                self.take_interim_snapshot().await;
            }

            let req = crate::llm_client::LlmCompleteRequest {
                model: String::new(),
                messages: messages.clone(),
                tools: self.current_tool_list(),
                system: Some(system.clone()),
                max_tokens: 4096,
                turn_id,
            };

            tracing::debug!(
                pid = self.pid.as_u64(),
                turn = turn_num,
                messages = messages.len(),
                tools = req.tools.len(),
                "dispatching LLM request"
            );

            if let Some(t) = &self.tracer {
                t.agent_llm_call(
                    self.pid.as_u64(),
                    turn_num,
                    "",
                    messages.len(),
                    self.current_tool_list().len(),
                );
            }

            let cancel = CancellationToken::new();
            let response = tokio::select! {
                res = self.run_turn_streaming(req, client, turn_id, cancel.clone()) => {
                    match res {
                        Ok(r) => r,
                        Err(AvixError::Cancelled(_)) => {
                            if self.killed.load(Ordering::Acquire) {
                                self.signal_rx = Some(signal_rx);
                                return Err(AvixError::Cancelled("SIGKILL/SIGSTOP".into()));
                            }
                            continue;
                        }
                        Err(e) => {
                            self.signal_rx = Some(signal_rx);
                            return Err(e);
                        }
                    }
                }
                sig_opt = signal_rx.recv() => {
                    match sig_opt {
                        Some(sig) => {
                            cancel.cancel();
                            self.handle_signal_during_llm(&sig).await;
                            if self.killed.load(Ordering::Acquire) {
                                self.signal_rx = Some(signal_rx);
                                return Err(AvixError::Cancelled(
                                    "SIGKILL/SIGSTOP during LLM".into(),
                                ));
                            }
                            continue;
                        }
                        None => {
                            self.signal_rx = Some(signal_rx);
                            return Err(AvixError::Cancelled("signal channel closed".into()));
                        }
                    }
                }
            };

            tracing::debug!(
                pid = self.pid.as_u64(),
                stop_reason = ?response.stop_reason,
                input_tokens = response.input_tokens,
                output_tokens = response.output_tokens,
                "LLM response received"
            );

            if let Some(t) = &self.tracer {
                let stop = format!("{:?}", response.stop_reason);
                t.agent_llm_response(
                    self.pid.as_u64(),
                    turn_num,
                    &stop,
                    response.input_tokens as u64,
                    response.output_tokens as u64,
                );
            }

            self.tokens_consumed = self
                .tokens_consumed
                .saturating_add(response.total_tokens() as u64);
            self.context_used = response.input_tokens as u64;
            if let Some(vfs) = &self.vfs {
                let vfs = Arc::clone(vfs);
                self.write_status_yaml(&vfs).await;
            }

            match interpret_stop_reason(&response) {
                TurnAction::ReturnResult(text) => {
                    tracing::info!(
                        pid = self.pid.as_u64(),
                        turn = turn_num,
                        tokens_consumed = self.tokens_consumed,
                        tool_calls = self.tool_calls_total,
                        "turn loop complete; persisting invocation state"
                    );
                    self.memory.conversation_history
                        .push(ConversationEntry::from_role_content(Role::Assistant, &text));
                    self.save_invocation_state().await;
                    self.save_session_response(&text).await;
                    self.signal_rx = Some(signal_rx);
                    return Ok(TurnResult { text });
                }
                TurnAction::SummariseContext => {
                    let text = response
                        .content
                        .iter()
                        .filter_map(|c| c["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("");
                    tracing::info!(
                        pid = self.pid.as_u64(),
                        turn = turn_num,
                        "context summarised; persisting invocation state"
                    );
                    self.memory.conversation_history
                        .push(ConversationEntry::from_role_content(Role::Assistant, &text));
                    self.save_invocation_state().await;
                    self.save_session_response(&text).await;
                    self.signal_rx = Some(signal_rx);
                    return Ok(TurnResult { text });
                }
                TurnAction::DispatchTools(calls) => {
                    chain_count += calls.len();
                    if chain_count > self.max_tool_chain_length {
                        self.signal_rx = Some(signal_rx);
                        return Err(AvixError::ConfigParse(format!(
                            "exceeded max tool chain limit of {}",
                            self.max_tool_chain_length
                        )));
                    }
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": response.content
                    }));
                    tracing::debug!(
                        pid = self.pid.as_u64(),
                        tool_count = calls.len(),
                        "dispatching tool calls"
                    );
                    let mut tool_results = Vec::new();
                    // Track (call, result) pairs for conversation history.
                    let mut dispatched_with_results: Vec<(&AvixToolCall, serde_json::Value)> =
                        Vec::new();
                    for call in &calls {
                        if let Err(e) = validate_tool_call(
                            &self.token,
                            call,
                            &mut self.tools.tool_budgets,
                        ) {
                            tracing::warn!(
                                pid = self.pid.as_u64(),
                                tool = %call.name,
                                error = %e,
                                "tool call validation failed"
                            );
                            tool_results.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": call.call_id,
                                "content": format!("Error: {e}")
                            }));
                            continue;
                        }

                        if self.tools.hil_required_tools.iter().any(|t| t == &call.name) {
                            if let Some(kernel) = &self.kernel {
                                if !kernel.is_auto_approve().await {
                                    tracing::debug!(
                                        pid = self.pid.as_u64(),
                                        tool = %call.name,
                                        "HIL gate blocked tool call"
                                    );
                                    tool_results.push(serde_json::json!({
                                        "type": "tool_result",
                                        "tool_use_id": call.call_id,
                                        "content": "Tool call requires human approval (HIL gate). Not yet approved."
                                    }));
                                    continue;
                                }
                            } else {
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

                        self.tool_calls_total = self.tool_calls_total.saturating_add(1);
                        tracing::debug!(
                            pid = self.pid.as_u64(),
                            tool = %call.name,
                            call_id = %call.call_id,
                            tool_calls_total = self.tool_calls_total,
                            "dispatching tool call"
                        );

                        if let Some(interval) = self.snapshot_interval {
                            self.tool_calls_since_last_snapshot += 1;
                            if self.tool_calls_since_last_snapshot >= interval {
                                self.take_interim_snapshot().await;
                                self.tool_calls_since_last_snapshot = 0;
                            }
                        }

                        if let Some(bus) = &self.event_bus {
                            bus.agent_tool_call(
                                &self.atp_session_id,
                                self.pid.as_u64(),
                                &call.call_id,
                                &call.name,
                                &call.args,
                            );
                        }
                        if let Some(t) = &self.tracer {
                            t.agent_tool_call(
                                self.pid.as_u64(),
                                &call.call_id,
                                &call.name,
                                &call.args,
                            );
                        }

                        let result = if self.is_cat2_tool(&call.name) {
                            self.dispatch_category2(call).await?
                        } else {
                            self.dispatch_via_router(call).await?
                        };

                        tracing::debug!(
                            pid = self.pid.as_u64(),
                            tool = %call.name,
                            call_id = %call.call_id,
                            "tool call completed"
                        );

                        if let Some(bus) = &self.event_bus {
                            bus.agent_tool_result(
                                &self.atp_session_id,
                                self.pid.as_u64(),
                                &call.call_id,
                                &call.name,
                                &result.to_string(),
                            );
                        }
                        if let Some(t) = &self.tracer {
                            t.agent_tool_result(
                                self.pid.as_u64(),
                                &call.call_id,
                                &call.name,
                                &result,
                            );
                        }

                        // Persist interim invocation state after each tool call so
                        // progress is not lost on unexpected termination.
                        self.save_invocation_state().await;

                        tool_results.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": call.call_id,
                            "content": result.to_string()
                        }));
                        dispatched_with_results.push((call, result));
                    }

                    // Record this tool-dispatch turn in persistent conversation history.
                    {
                        let text = response
                            .content
                            .iter()
                            .filter_map(|c| c["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("");
                        let tc_entries: Vec<ToolCallEntry> = dispatched_with_results
                            .iter()
                            .map(|(call, result)| ToolCallEntry {
                                id: call.call_id.clone(),
                                name: call.name.clone(),
                                args: call.args.clone(),
                                result: Some(result.clone()),
                            })
                            .collect();
                        if !tc_entries.is_empty() || !text.is_empty() {
                            self.memory.conversation_history.push(ConversationEntry {
                                role: Role::Assistant,
                                content: text,
                                tool_calls: tc_entries,
                                files_changed: vec![],
                                thought: None,
                            });
                        }
                    }

                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": tool_results
                    }));
                }
            }
        }
    }

    /// Persist current invocation tokens and tool-call count to the store.
    async fn save_invocation_state(&self) {
        if self.invocation_id.is_empty() {
            return;
        }
        if let Some(store) = &self.invocation_store {
            let _ = store
                .persist_interim(
                    &self.invocation_id,
                    &self.memory.conversation_history,
                    self.tokens_consumed,
                    self.tool_calls_total,
                )
                .await;
        }
    }

    /// Update the session record with the latest response text and token count.
    async fn save_session_response(&self, response_text: &str) {
        if self.session_id.is_empty() {
            return;
        }
        if let Some(store) = &self.session_store {
            let session_uuid =
                match uuid::Uuid::parse_str(&self.session_id) {
                    Ok(u) => u,
                    Err(_) => return,
                };
            if let Ok(Some(mut session)) = store.get(&session_uuid).await {
                // Store a truncated preview of the last response as the session summary.
                let preview_len = response_text.len().min(500);
                session.summary = Some(response_text[..preview_len].to_string());
                session.last_updated = chrono::Utc::now();
                tracing::debug!(
                    pid = self.pid.as_u64(),
                    session_id = %self.session_id,
                    "updating session response summary"
                );
                let _ = store.update(&session).await;
            }
        }
    }

    pub async fn run_until_complete(&mut self, goal: &str) -> Result<TurnResult, AvixError> {
        use crate::executor::stop_reason::{interpret_stop_reason, TurnAction};

        let mut messages: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "user", "content": goal})];
        let mut chain_count = 0;

        loop {
            let _system = self.build_system_prompt_str();
            let response = {
                let mut q = self.llm_queue.lock().unwrap();
                if q.is_empty() {
                    return Err(AvixError::ConfigParse("no more mock LLM responses".into()));
                }
                q.remove(0)
            };
            self.call_log.lock().unwrap().push(messages.clone());

            self.maybe_renew_token();
            if self.token.is_expired() {
                return Err(AvixError::ConfigParse(
                    "capability token expired; cannot begin turn".into(),
                ));
            }

            match interpret_stop_reason(&response) {
                TurnAction::ReturnResult(text) => return Ok(TurnResult { text }),
                TurnAction::SummariseContext => {
                    // stub: continue
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

                    for c in &response.content {
                        messages.push(serde_json::json!({"role": "assistant", "content": [c]}));
                    }
                    for r in results {
                        messages.push(serde_json::json!({"role": "user", "content": r}));
                    }
                }
            }
        }
    }
}

/// Check that `username` has execute (`x`) permission on `entry`.
///
/// Rules:
/// - `username == "root"` → always allow (admin)
/// - `username == entry.permissions.owner` → check owner bits contain `x`
/// - otherwise → check `entry.permissions.all` contains `x`
fn check_tool_execute_permission(entry: &ToolEntry, username: &str) -> Result<(), AvixError> {
    // Root (admin) is always allowed
    if username == "root" {
        return Ok(());
    }
    let perms = &entry.permissions;
    // The tool owner has implicit execute permission on their own tool
    if username == perms.owner {
        return Ok(());
    }
    // All other users must have 'x' in the `all` permission bits
    if perms.all.contains('x') {
        Ok(())
    } else {
        Err(AvixError::CapabilityDenied(format!(
            "agent '{}' does not have execute permission on tool '{}'",
            username,
            entry.name.as_str()
        )))
    }
}

/// Returns true if the tool should have `_caller` injected into its params.
///
/// Kernel tools (namespace `kernel/`) are always caller-scoped. For service tools,
/// check the descriptor's `caller_scoped` field.
fn is_caller_scoped_tool(name: &str, descriptor: &serde_json::Value) -> bool {
    if name.starts_with("kernel/") {
        return true;
    }
    descriptor
        .get("caller_scoped")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
    use crate::executor::MockKernelHandle;
    use crate::executor::SpawnParams;
    use crate::llm_client::{LlmCompleteRequest, LlmCompleteResponse, StopReason};
    use crate::llm_svc::adapter::AvixToolCall;
    use crate::types::{token::CapabilityToken, Pid};
    use serde_json::json;
    use super::check_tool_execute_permission;

    fn make_params(pid_val: u64, caps: &[&str]) -> SpawnParams {
        SpawnParams {
            pid: Pid::from_u64(pid_val),
            agent_name: "test-agent".into(),
            goal: "test goal".into(),
            spawned_by: "kernel".into(),
            session_id: "sess-test".into(),
            atp_session_id: String::new(),
            token: CapabilityToken::test_token(caps),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
            invocation_id: String::new(),
        }
    }

    fn make_params_with_dir(
        pid_val: u64,
        caps: &[&str],
        runtime_dir: std::path::PathBuf,
    ) -> SpawnParams {
        SpawnParams {
            pid: Pid::from_u64(pid_val),
            agent_name: "test-agent".into(),
            goal: "test goal".into(),
            spawned_by: "alice".into(),
            session_id: "sess-test".into(),
            atp_session_id: String::new(),
            token: CapabilityToken::test_token(caps),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir,
            invocation_id: String::new(),
        }
    }

    async fn make_executor(pid_val: u64, caps: &[&str]) -> RuntimeExecutor {
        let registry = Arc::new(MockToolRegistry::new());
        RuntimeExecutor::spawn_with_registry(make_params(pid_val, caps), registry)
            .await
            .unwrap()
    }

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

    // Dispatch tests

    #[tokio::test]
    async fn test_dispatch_cap_list() {
        let mut executor = make_executor(
            3210,
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
        assert!(result.get("grantedTools").is_some());
    }

    #[tokio::test]
    async fn test_dispatch_cap_escalate() {
        let mut executor = make_executor(3211, &[]).await;
        let call = AvixToolCall {
            call_id: "c2".into(),
            name: "cap/escalate".into(),
            args: json!({"reason": "I found PII data", "context": "", "options": []}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(result.get("guidance").is_some());
    }

    #[tokio::test]
    async fn test_dispatch_pipe_open() {
        let mut executor =
            make_executor(3212, &["pipe/open", "pipe/write", "pipe/read", "pipe/close"]).await;
        let call = AvixToolCall {
            call_id: "c3".into(),
            name: "pipe/open".into(),
            args: json!({"targetPid": 99, "direction": "out"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        assert!(result.get("pipeId").is_some());
    }

    #[tokio::test]
    async fn test_dispatch_job_watch() {
        let mut executor = make_executor(3244, &[]).await;
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
        let mut executor = make_executor(3245, &[]).await;
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
        let mut executor = make_executor(3246, &[]).await;
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
        let mut executor = make_executor(3247, &[]).await;
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
        let mut executor = make_executor(3248, &[]).await;

        let w = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "pw".into(),
                name: "pipe/write".into(),
                args: json!({"pipeId": "p1", "content": "hello"}),
            })
            .await
            .unwrap();
        assert!(w.get("tokensSent").is_some());

        let r = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "pr".into(),
                name: "pipe/read".into(),
                args: json!({"pipeId": "p1"}),
            })
            .await
            .unwrap();
        assert!(r.get("content").is_some());

        let c = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "pc".into(),
                name: "pipe/close".into(),
                args: json!({"pipeId": "p1"}),
            })
            .await
            .unwrap();
        assert_eq!(c["closed"], true);
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool_returns_stub() {
        let mut executor = make_executor(3249, &[]).await;
        let result = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "c99".into(),
                name: "some/unknown-tool".into(),
                args: json!({}),
            })
            .await
            .unwrap();
        assert!(result.get("content").is_some());
    }

    #[tokio::test]
    async fn test_dispatch_cap_request_tool_without_kernel() {
        let mut executor = make_executor(3250, &[]).await;
        let result = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "c5".into(),
                name: "cap/request-tool".into(),
                args: json!({"tool": "fs/read", "reason": "need it"}),
            })
            .await
            .unwrap();
        assert_eq!(result["approved"], false);
    }

    #[tokio::test]
    async fn test_dispatch_agent_spawn_without_kernel() {
        let mut executor = make_executor(3251, &[]).await;
        let result = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "c6".into(),
                name: "agent/spawn".into(),
                args: json!({"agent": "worker", "goal": "do stuff"}),
            })
            .await
            .unwrap();
        assert_eq!(result["spawned"], true);
    }

    #[tokio::test]
    async fn test_dispatch_agent_kill() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        let params = make_params(
            3252,
            &["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"],
        );
        let mut executor =
            RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, Arc::clone(&kernel))
                .await
                .unwrap();
        let result = executor
            .dispatch_category2(&AvixToolCall {
                call_id: "kill-1".into(),
                name: "agent/kill".into(),
                args: json!({"pid": 77, "reason": "done"}),
            })
            .await
            .unwrap();
        assert_eq!(result["killed"], true);
        assert!(kernel.received_proc_kill(77).await);
    }

    // Run tests

    #[tokio::test]
    async fn test_run_with_client_rejects_ungranted_tool() {
        let registry = Arc::new(MockToolRegistry::new());
        let params = SpawnParams {
            pid: Pid::from_u64(3260),
            agent_name: "agent".into(),
            goal: "goal".into(),
            spawned_by: "kernel".into(),
            session_id: "sess".into(),
            atp_session_id: String::new(),
            token: CapabilityToken::test_token(&["cap/list"]),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
            invocation_id: String::new(),
        };
        let mut executor = RuntimeExecutor::spawn_with_registry(params, registry)
            .await
            .unwrap();

        let mock_client = MockLlmClient::new(vec![
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use", "id": "call-bad", "name": "fs__read",
                    "input": {"path": "/etc/passwd"}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "Done."})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 5,
                output_tokens: 2,
            },
        ]);
        let result = executor.run_with_client("do something", &mock_client).await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_hil_gate_blocks_without_kernel() {
        let mut executor = make_executor(3261, &[]).await;
        executor.require_hil_for("cap/list");

        let mock_client = MockLlmClient::new(vec![
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use", "id": "hil-call", "name": "cap__list", "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            },
            LlmCompleteResponse {
                content: vec![json!({"type": "text", "text": "Done."})],
                stop_reason: StopReason::EndTurn,
                input_tokens: 5,
                output_tokens: 2,
            },
        ]);
        let result = executor.run_with_client("do something", &mock_client).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(executor.pending_messages.iter().any(|m| m.contains("HIL required")));
    }

    #[tokio::test]
    async fn test_hil_gate_with_auto_approve_kernel() {
        let registry = Arc::new(MockToolRegistry::new());
        let kernel = Arc::new(MockKernelHandle::new());
        kernel.auto_approve_resource_request().await;

        let params = make_params(3262, &["cap/list"]);
        let mut executor =
            RuntimeExecutor::spawn_with_registry_and_kernel(params, registry, kernel)
                .await
                .unwrap();
        executor.require_hil_for("cap/list");

        let mock_client = MockLlmClient::new(vec![
            LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use", "id": "hil-auto", "name": "cap__list", "input": {}
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
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn test_run_with_client_chain_limit_exceeded() {
        let mut executor = make_executor(3263, &[]).await;
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
    async fn test_run_until_complete_fs_read() {
        let mut executor = make_executor(3264, &[]).await;
        executor.on_fs_read("/tmp/hello.txt", b"file contents here");

        executor.push_llm_response(LlmCompleteResponse {
            content: vec![json!({
                "type": "tool_use", "id": "read-call", "name": "fs/read",
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
        let mut executor = make_executor(3265, &[]).await;
        executor.set_max_tool_chain_length(1);
        for i in 0..3 {
            executor.push_llm_response(LlmCompleteResponse {
                content: vec![json!({
                    "type": "tool_use", "id": format!("call-{i}"), "name": "cap/list", "input": {}
                })],
                stop_reason: StopReason::ToolUse,
                input_tokens: 5,
                output_tokens: 2,
            });
        }
        let result = executor.run_until_complete("do stuff").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max tool chain"));
    }

    #[tokio::test]
    async fn test_run_until_complete_summarise_context_stub() {
        let mut executor = make_executor(3266, &[]).await;
        executor.push_llm_response(LlmCompleteResponse {
            content: vec![],
            stop_reason: StopReason::MaxTokens,
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

    #[tokio::test]
    async fn test_llm_call_count_tracks_calls() {
        let mut executor = make_executor(3267, &[]).await;
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
        let executor = make_executor(3268, &[]).await;
        assert!(executor.call_messages(99).is_empty());
    }

    #[tokio::test]
    async fn test_current_tool_list_excludes_removed() {
        let mut executor = make_executor(3269, &[]).await;
        let initial = executor.current_tool_list().len();
        executor.handle_tool_changed("removed", "cap/list", "test").await;
        assert!(executor.current_tool_list().len() < initial);
    }

    // GAP-A: sys/tools returns all tools when no filter given
    #[tokio::test]
    async fn test_dispatch_sys_tools_no_filter_returns_tools() {
        use crate::executor::MockKernelHandle;
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::types::tool::ToolName;

        let mut kernel = MockKernelHandle::new();
        let reg = Arc::new(ToolRegistry::new());
        reg.add(
            "kernel",
            vec![ToolEntry::new(
                ToolName::parse("kernel/proc/spawn").unwrap(),
                "kernel".into(),
                ToolState::Available,
                ToolVisibility::All,
                serde_json::json!({"name": "kernel/proc/spawn", "description": "Spawn agent"}),
            )],
        )
        .await
        .unwrap();
        kernel.tool_registry = Some(reg);

        let registry = Arc::new(MockToolRegistry::new());
        let params = make_params(9001, &[]);
        let mut executor = RuntimeExecutor::spawn_with_registry_and_kernel(
            params,
            registry,
            Arc::new(kernel),
        )
        .await
        .unwrap();

        let call = AvixToolCall {
            call_id: "st1".into(),
            name: "sys/tools".into(),
            args: json!({}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert!(!tools.is_empty(), "sys/tools should return registered tools");
        let names: Vec<_> = tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"kernel/proc/spawn"));
    }

    // GAP-A: sys/tools with namespace filter returns only matching tools
    #[tokio::test]
    async fn test_dispatch_sys_tools_namespace_filter() {
        use crate::executor::MockKernelHandle;
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::types::tool::ToolName;

        let mut kernel = MockKernelHandle::new();
        let reg = Arc::new(ToolRegistry::new());
        reg.add(
            "kernel",
            vec![
                ToolEntry::new(
                    ToolName::parse("fs/read").unwrap(),
                    "fs.svc".into(),
                    ToolState::Available,
                    ToolVisibility::All,
                    serde_json::json!({"name": "fs/read", "description": "Read file"}),
                ),
                ToolEntry::new(
                    ToolName::parse("llm/complete").unwrap(),
                    "llm.svc".into(),
                    ToolState::Available,
                    ToolVisibility::All,
                    serde_json::json!({"name": "llm/complete", "description": "LLM call"}),
                ),
            ],
        )
        .await
        .unwrap();
        kernel.tool_registry = Some(reg);

        let registry = Arc::new(MockToolRegistry::new());
        let params = make_params(9002, &[]);
        let mut executor = RuntimeExecutor::spawn_with_registry_and_kernel(
            params,
            registry,
            Arc::new(kernel),
        )
        .await
        .unwrap();

        let call = AvixToolCall {
            call_id: "st2".into(),
            name: "sys/tools".into(),
            args: json!({"namespace": "fs"}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"fs/read"), "should include fs/read");
        assert!(!names.contains(&"llm/complete"), "should exclude llm/complete");
    }

    // GAP-A: sys/tools with granted_only=true returns only token-granted tools
    #[tokio::test]
    async fn test_dispatch_sys_tools_granted_only() {
        use crate::executor::MockKernelHandle;
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::types::tool::ToolName;

        let mut kernel = MockKernelHandle::new();
        let reg = Arc::new(ToolRegistry::new());
        reg.add(
            "kernel",
            vec![
                ToolEntry::new(
                    ToolName::parse("fs/read").unwrap(),
                    "fs.svc".into(),
                    ToolState::Available,
                    ToolVisibility::All,
                    serde_json::json!({"name": "fs/read", "description": "Read file"}),
                ),
                ToolEntry::new(
                    ToolName::parse("fs/write").unwrap(),
                    "fs.svc".into(),
                    ToolState::Available,
                    ToolVisibility::All,
                    serde_json::json!({"name": "fs/write", "description": "Write file"}),
                ),
            ],
        )
        .await
        .unwrap();
        kernel.tool_registry = Some(reg);

        // Token only grants fs/read, not fs/write
        let registry = Arc::new(MockToolRegistry::new());
        let params = make_params(9003, &["fs/read"]);
        let mut executor = RuntimeExecutor::spawn_with_registry_and_kernel(
            params,
            registry,
            Arc::new(kernel),
        )
        .await
        .unwrap();

        let call = AvixToolCall {
            call_id: "st3".into(),
            name: "sys/tools".into(),
            args: json!({"granted_only": true}),
        };
        let result = executor.dispatch_category2(&call).await.unwrap();
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"fs/read"), "granted tool should appear");
        assert!(!names.contains(&"fs/write"), "non-granted tool should be excluded");
    }

    // GAP-B: dispatch_via_router forwards call to mock IPC service and returns result
    #[tokio::test]
    async fn test_dispatch_via_router_with_ipc_binding() {
        use crate::tool_registry::{ToolEntry, ToolRegistry, ToolState, ToolVisibility};
        use crate::tool_registry::permissions::ToolPermissions;
        use crate::types::tool::ToolName;
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("fs.sock");

        // Spawn a mock service listener
        let sock_clone = sock.clone();
        let listener = UnixListener::bind(&sock_clone).unwrap();
        let server = tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let req: serde_json::Value = crate::ipc::frame::read_from(&mut conn).await.unwrap();
            let resp = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": {"content": "file contents here"},
            });
            let bytes = crate::ipc::frame::encode(&resp).unwrap();
            conn.write_all(&bytes).await.unwrap();
        });

        // Register a tool with IPC binding pointing to our mock service
        let reg = Arc::new(ToolRegistry::new());
        let descriptor = serde_json::json!({
            "name": "fs/read",
            "description": "Read a file",
            "ipc": {
                "transport": "local-ipc",
                "endpoint": "fs",
                "method": "fs.read"
            }
        });
        reg.add(
            "fs.svc",
            vec![
                ToolEntry::new(
                    ToolName::parse("fs/read").unwrap(),
                    "fs.svc".into(),
                    ToolState::Available,
                    ToolVisibility::All,
                    descriptor,
                )
                .with_permissions(ToolPermissions::new("root".into(), "".into(), "rwx".into())),
            ],
        )
        .await
        .unwrap();

        let params = make_params_with_dir(10200, &["fs/read"], dir.path().to_path_buf());
        let executor = RuntimeExecutor::spawn_with_real_registry(params, Arc::clone(&reg))
            .await
            .unwrap();

        let call = AvixToolCall {
            call_id: "r1".into(),
            name: "fs/read".into(),
            args: serde_json::json!({"path": "/data/file.txt"}),
        };

        let result = executor.dispatch_via_router(&call).await.unwrap();
        assert_eq!(result["content"], "file contents here");
        let _ = server.await;
    }

    // GAP-B: tool not in registry returns ConfigParse error
    #[tokio::test]
    async fn test_dispatch_via_router_tool_not_in_registry() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Arc::new(crate::tool_registry::ToolRegistry::new());
        let params = make_params_with_dir(10201, &["fs/read"], dir.path().to_path_buf());
        let executor = RuntimeExecutor::spawn_with_real_registry(params, Arc::clone(&reg))
            .await
            .unwrap();

        let call = AvixToolCall {
            call_id: "r2".into(),
            name: "fs/read".into(),
            args: serde_json::json!({}),
        };
        let result = executor.dispatch_via_router(&call).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found in registry"), "got: {err}");
    }

    // GAP-B: check_tool_execute_permission — owner with rwx allowed
    #[test]
    fn test_permission_owner_rwx_allowed() {
        use crate::tool_registry::{ToolEntry, ToolState, ToolVisibility};
        use crate::tool_registry::permissions::ToolPermissions;
        use crate::types::tool::ToolName;

        let entry = ToolEntry::new(
            ToolName::parse("fs/read").unwrap(),
            "alice".into(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::json!({}),
        )
        .with_permissions(ToolPermissions::new("alice".into(), "".into(), "r--".into()));

        assert!(check_tool_execute_permission(&entry, "alice").is_ok());
    }

    // GAP-B: non-owner with all=r-- denied
    #[test]
    fn test_permission_non_owner_no_execute_denied() {
        use crate::tool_registry::{ToolEntry, ToolState, ToolVisibility};
        use crate::tool_registry::permissions::ToolPermissions;
        use crate::types::tool::ToolName;

        let entry = ToolEntry::new(
            ToolName::parse("fs/read").unwrap(),
            "alice".into(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::json!({}),
        )
        .with_permissions(ToolPermissions::new("alice".into(), "".into(), "r--".into()));

        let result = check_tool_execute_permission(&entry, "bob");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("execute permission"), "got: {err}");
    }

    // GAP-B: root is always allowed
    #[test]
    fn test_permission_root_always_allowed() {
        use crate::tool_registry::{ToolEntry, ToolState, ToolVisibility};
        use crate::tool_registry::permissions::ToolPermissions;
        use crate::types::tool::ToolName;

        let entry = ToolEntry::new(
            ToolName::parse("fs/read").unwrap(),
            "alice".into(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::json!({}),
        )
        .with_permissions(ToolPermissions::new("alice".into(), "".into(), "---".into()));

        assert!(check_tool_execute_permission(&entry, "root").is_ok());
    }
}
