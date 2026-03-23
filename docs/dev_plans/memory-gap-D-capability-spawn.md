# Memory Gap D — Capability Grants, Spawn Injection & Session Auto-Log

> **Status:** Complete
> **Priority:** High — agents cannot use memory tools without capability grants at spawn
> **Depends on:** memory-gap-A (schema), memory-gap-B (VFS layout), memory-gap-C (service tools)
> **Affects:** `avix-core/src/types/token.rs`, `avix-core/src/executor/runtime_executor.rs`, `avix-core/src/params/resolver.rs`

---

## Problem

1. `CapabilityToolMap` has no `memory:read`, `memory:write`, or `memory:share` entries.
   Agents cannot be granted memory tools at spawn.
2. `RuntimeExecutor::spawn_with_registry()` does not call `init_user_memory_tree()` or
   inject memory context into the system prompt.
3. `RuntimeExecutor`'s SIGSTOP handler does not call `memory/log-event` when
   `autoLogOnSessionEnd: true`.
4. The `resolved.yaml` written to `/proc/<pid>/resolved.yaml` does not include the
   `ResolvedMemory` block.

---

## What Needs to Be Built

### 1. Add memory capabilities to `CapabilityToolMap`

In `crates/avix-core/src/types/token.rs` (or wherever `CapabilityToolMap::all_gated_cat2_tools()` lives):

```rust
// Existing entries (unchanged):
("agent:spawn", vec!["agent/spawn", "agent/kill", "agent/list", "agent/wait", "agent/send-message"]),
("pipe:use",    vec!["pipe/open", "pipe/write", "pipe/read", "pipe/close"]),
// ...

// New memory entries:
("memory:read",  vec!["memory/retrieve", "memory/get-fact", "memory/get-preferences"]),
("memory:write", vec![
    // memory:write includes all memory:read tools (cumulative grant)
    "memory/retrieve", "memory/get-fact", "memory/get-preferences",
    "memory/log-event", "memory/store-fact", "memory/update-preference", "memory/forget",
]),
("memory:share", vec!["memory/share-request"]),
```

> `memory:write` includes the read tools because the spec says `memory:write` grants all
> `memory:read` tools plus write tools. Implementing this as a superset in `CapabilityToolMap`
> is cleaner than special-casing in the ACL check.

### 2. Add `memory:share` always-present tool

Per the spec, `memory/share-request` is privilege-level and never granted by default.
It is listed in `CapabilityToolMap` as `memory:share` — only granted when the agent
manifest explicitly sets `sharing.canRequest: true` and an operator grants it.

### 3. Update `ResolvedConfig` YAML output

Add `ResolvedMemory` to the `ResolvedConfig` struct and include it in the
`/proc/<pid>/resolved.yaml` file written at spawn:

```rust
// In params/resolver.rs
pub struct ResolvedConfig {
    // ... existing fields
    pub memory: ResolvedMemory,
}
```

The `ResolvedMemory` block is resolved by applying the agent manifest's memory block
against the kernel config's memory defaults (same resolution pattern as other blocks).

### 4. `RuntimeExecutor::spawn_with_registry()` — memory init

After VFS setup and before `SIGSTART`, add:

```rust
// Init memory tree for this agent if memory is enabled
if self.resolved_memory.episodic_enabled || self.resolved_memory.semantic_enabled {
    if let Err(e) = init_user_memory_tree(&self.vfs, &self.spawned_by, &self.agent_name).await {
        tracing::warn!(pid = self.pid.as_u32(), err = ?e, "memory tree init failed");
    }
}

// Inject memory context into system prompt if configured
if self.resolved_memory.preferences_enabled && self.resolved_memory.auto_inject_at_spawn {
    match self.build_memory_context_block().await {
        Ok(Some(block)) => {
            self.system_prompt = format!("{}\n\n{}", block, self.system_prompt);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(pid = self.pid.as_u32(), err = ?e, "memory context injection failed"),
    }
}
```

### 5. `build_memory_context_block()` — assemble the injection block

```rust
async fn build_memory_context_block(&self) -> Result<Option<String>, AvixError> {
    let mut parts = vec![];

    // 1. User preferences
    let pref_path = UserPreferenceModel::vfs_path(&self.spawned_by, &self.agent_name);
    if let Ok(bytes) = self.vfs.read(&VfsPath::parse(&pref_path).unwrap()).await {
        if let Ok(model) = UserPreferenceModel::from_yaml(&String::from_utf8_lossy(&bytes)) {
            let mut pref_text = format!("User preferences:\n  {}", model.spec.summary);
            if !model.spec.corrections.is_empty() {
                pref_text.push_str("\n\n  Corrections to avoid repeating:");
                for c in &model.spec.corrections {
                    pref_text.push_str(&format!("\n    • \"{}\" ({})",
                        c.correction,
                        c.at.format("%Y-%m-%d")
                    ));
                }
            }
            parts.push(pref_text);
        }
    }

    // 2. Recent episodic context (N most recent, configurable)
    let episodic_dir = format!(
        "/users/{}/memory/{}/episodic",
        self.spawned_by, self.agent_name
    );
    let n = 5usize; // kernel_config.memory.spawn.episodic_context_records
    let records = memory_svc::store::list_records(&self.vfs, &episodic_dir, ...).await
        .unwrap_or_default();
    // Sort by created_at descending, take N
    let mut sorted = records;
    sorted.sort_by(|a, b| b.metadata.created_at.cmp(&a.metadata.created_at));
    let recent: Vec<_> = sorted.into_iter().take(n).collect();
    if !recent.is_empty() {
        let mut hist = format!("Recent session history (last {}):", recent.len());
        for r in &recent {
            let outcome = r.spec.outcome.as_ref()
                .map(|o| format!("[{:?}]", o).to_lowercase())
                .unwrap_or_default();
            hist.push_str(&format!("\n  • {} {} {}",
                r.metadata.created_at.format("%Y-%m-%d"),
                outcome,
                &r.spec.content[..120.min(r.spec.content.len())]
            ));
        }
        parts.push(hist);
    }

    // 3. Pinned facts
    let semantic_dir = format!(
        "/users/{}/memory/{}/semantic",
        self.spawned_by, self.agent_name
    );
    let all_semantic = memory_svc::store::list_records(&self.vfs, &semantic_dir, ...).await
        .unwrap_or_default();
    let pinned: Vec<_> = all_semantic.into_iter().filter(|r| r.metadata.pinned).collect();
    if !pinned.is_empty() {
        let mut pin_text = "Pinned facts:".to_string();
        for r in &pinned {
            let key = r.spec.key.as_deref().unwrap_or(&r.metadata.id);
            pin_text.push_str(&format!("\n  • {}: {}",
                key,
                &r.spec.content[..120.min(r.spec.content.len())]
            ));
        }
        parts.push(pin_text);
    }

    if parts.is_empty() {
        return Ok(None);
    }

    let block = format!(
        "[MEMORY CONTEXT — {} — injected by memory.svc]\n\n{}",
        self.agent_name,
        parts.join("\n\n")
    );
    Ok(Some(block))
}
```

### 6. SIGSTOP handler — auto-log session end

When `autoLogOnSessionEnd: true`, the SIGSTOP handler must:
1. Ask the LLM to produce a session summary (using the existing `conversation_history`)
2. Call `memory/log-event` via the internal service dispatch

```rust
"SIGSTOP" => {
    if self.resolved_memory.auto_log_on_session_end
        && !self.conversation_history.is_empty()
    {
        self.auto_log_session_end().await;
    }
    // existing SIGSTOP handling continues...
}
```

```rust
async fn auto_log_session_end(&self) {
    // Build a summarisation prompt from conversation_history
    // The full conversation_history is in-memory — pass it to llm/complete
    // to get a summary. This is the only place where conversation_history
    // is used to produce a memory record.
    let summary_prompt = format!(
        "You have just completed a session. Summarise what was accomplished, \
         what the user asked, key findings or decisions, outcomes, and any \
         follow-up actions the user requested. Be concise (2-5 sentences). \
         Do not include raw tool outputs — only meaningful outcomes.\n\n\
         Session transcript:\n{}",
        self.conversation_history.iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Call llm/complete via the executor's IPC client
    // (same path as any other llm/complete call)
    match self.llm_summarise(&summary_prompt).await {
        Ok(summary) => {
            let caller = CallerContext {
                pid: self.pid.as_u32(),
                agent_name: self.agent_name.clone(),
                owner: self.spawned_by.clone(),
                session_id: self.session_id.clone(),
                granted_tools: self.token.granted_tools.clone(),
            };
            let params = json!({
                "summary": summary,
                "outcome": "success",
                "scope": "own"
            });
            if let Err(e) = self.memory_svc.dispatch("memory/log-event", params, &caller).await {
                tracing::warn!(pid = self.pid.as_u32(), err = ?e, "auto session log failed");
            }
        }
        Err(e) => tracing::warn!(pid = self.pid.as_u32(), err = ?e, "session summarisation failed"),
    }
}
```

> **Conversation history note:** `conversation_history` is the full turn-by-turn message
> list held in `RuntimeExecutor` memory. It is passed to every `llm/complete` call to
> preserve context (stateless LLM). It is NOT persisted to VFS or stored in memory.svc.
> Only the LLM-produced summary is stored as an episodic record. The raw history is
> discarded when the executor exits.

---

## TDD Test Plan

File: `crates/avix-core/tests/memory_spawn.rs` (new integration test file)

```rust
// T-MD-01: memory:read maps to retrieve/get-fact/get-preferences tools
#[test]
fn memory_read_capability_maps_correctly() {
    let tools = CapabilityToolMap::tools_for_capability("memory:read");
    assert!(tools.contains(&"memory/retrieve"));
    assert!(tools.contains(&"memory/get-fact"));
    assert!(tools.contains(&"memory/get-preferences"));
    assert!(!tools.contains(&"memory/log-event"), "log-event requires write");
}

// T-MD-02: memory:write is a superset of memory:read
#[test]
fn memory_write_includes_read_tools() {
    let tools = CapabilityToolMap::tools_for_capability("memory:write");
    assert!(tools.contains(&"memory/retrieve"));
    assert!(tools.contains(&"memory/log-event"));
    assert!(tools.contains(&"memory/store-fact"));
    assert!(tools.contains(&"memory/forget"));
}

// T-MD-03: agent spawned with memory:write gets all memory tools registered
#[tokio::test]
async fn spawn_with_memory_write_registers_tools() {
    let (executor, registry) = spawn_test_executor_with_caps(&["memory:write"]).await;
    let tools = registry.list_tools().await;
    assert!(tools.iter().any(|t| t.name == "memory/log-event"));
    assert!(tools.iter().any(|t| t.name == "memory/retrieve"));
}

// T-MD-04: resolved.yaml includes memory block
#[tokio::test]
async fn resolved_yaml_includes_memory_block() {
    let (executor, vfs) = spawn_test_executor("alice").await;
    let path = VfsPath::parse(&format!("/proc/{}/resolved.yaml", executor.pid())).unwrap();
    let bytes = vfs.read(&path).await.unwrap();
    let yaml = String::from_utf8(bytes).unwrap();
    assert!(yaml.contains("episodicEnabled"), "expected memory block in resolved.yaml");
}

// T-MD-05: memory tree is initialised at spawn when memory is enabled
#[tokio::test]
async fn spawn_creates_memory_tree() {
    let (executor, vfs) = spawn_test_executor_with_caps(&["memory:write"]).await;
    assert!(
        vfs.exists(&VfsPath::parse("/users/alice/memory/researcher/episodic/.keep").unwrap()).await,
        "expected episodic dir at spawn"
    );
}

// T-MD-06: system prompt includes memory context block when preferences exist
#[tokio::test]
async fn spawn_injects_memory_context_when_prefs_exist() {
    let (_, vfs) = setup_vfs_with_preferences("alice", "researcher").await;
    let executor = spawn_test_executor_with_vfs("alice", vfs.clone()).await;
    assert!(
        executor.system_prompt().contains("[MEMORY CONTEXT"),
        "expected memory context block in system prompt"
    );
}

// T-MD-07: SIGSTOP calls memory/log-event when autoLogOnSessionEnd is true
#[tokio::test]
async fn sigstop_auto_logs_session() {
    let (executor, vfs) = spawn_test_executor_with_caps(&["memory:write"]).await;
    // Add a fake conversation turn
    executor.push_conversation_message("user", "What is quantum computing?").await;
    executor.push_conversation_message("assistant", "Quantum computing uses qubits...").await;
    executor.deliver_signal("SIGSTOP").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // An episodic record should now exist
    let episodic_dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    let entries = vfs.list(&episodic_dir).await.unwrap();
    let yaml_entries: Vec<_> = entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(!yaml_entries.is_empty(), "expected episodic record after SIGSTOP");
}
```

---

## Implementation Notes

- The `llm_summarise()` helper in step 6 calls `llm/complete` via the existing
  `LlmClient` IPC path the executor already uses for inference. It passes
  `conversation_history` as the message list. No new infrastructure needed.
- The memory context block is prepended to the system prompt (before `goalTemplate`).
  This is consistent with the spec: "Injected block (written into system prompt before
  goalTemplate)."
- If `build_memory_context_block()` fails (e.g., VFS error), spawn continues normally
  with a `tracing::warn!`. Spawn must never abort because of a memory injection failure.
- The `memory.svc.dispatch()` call in `auto_log_session_end()` is a direct in-process
  call (not via IPC), since `memory.svc` is a module in `avix-core`. This avoids
  circular IPC dependency.

---

## Success Criteria

- [ ] `memory:read` maps to correct tools (T-MD-01)
- [ ] `memory:write` is a superset including read tools (T-MD-02)
- [ ] Agent with `memory:write` gets all memory tools registered (T-MD-03)
- [ ] `resolved.yaml` includes memory block (T-MD-04)
- [ ] Memory tree is created at spawn when memory is enabled (T-MD-05)
- [ ] System prompt includes context block when preferences exist (T-MD-06)
- [ ] SIGSTOP auto-logs session to episodic memory (T-MD-07)
- [ ] `cargo clippy --workspace -- -D warnings` passes
