# tool-visibility-gap-A — Registry Wiring & Dynamic Tool Discovery

**Status**: Complete  
**Last Updated**: 2026-04-12

---

## Summary

The `RuntimeExecutor` is completely disconnected from the real `ToolRegistry`. It uses
`MockToolRegistry` (a test fake) in production, meaning it has no access to tool descriptors,
IPC endpoints, or capability requirements at runtime. Consequently the LLM only ever sees
Cat2 tool descriptors built from a hard-coded match arm in `cat2_tool_descriptor()` — Cat1
service tools (e.g. `fs/read`, `fs/write`) granted in the capability token are invisible to
the agent.

This plan wires the real `ToolRegistry` into the executor and adds a `sys/tools` Cat2 tool
so agents can discover available tools on demand rather than having every registered tool
flooded into the LLM context every turn.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` — tool exposure model, Cat1/Cat2 split
- `docs/architecture/07-services.md` — `ipc.tool-add`, `ToolDescriptor`, `IpcBinding`
- `crates/avix-core/src/tool_registry/` — `ToolRegistry`, `ToolEntry`, `ToolScanner`
- `crates/avix-core/src/executor/` — `RuntimeExecutor`, `ToolManager`, `ToolRegistryHandle`
- `crates/avix-core/src/types/capability_map.rs` — Cat2 gated tool definitions

---

## Problems Being Fixed

| # | Problem | Location |
|---|---------|----------|
| 1 | `RuntimeExecutor` uses `MockToolRegistry` in production — no real descriptor access | `executor_factory.rs:94`, `runtime_executor.rs:45` |
| 2 | `ToolRegistryHandle` trait lacks lookup — only register/deregister | `runtime_executor.rs:32` |
| 3 | `current_tool_list()` never consults real registry — Cat1 tools invisible to LLM | `tool_manager.rs:32`, `dispatch_manager.rs:889` |
| 4 | `llm/*` tools are listed as Cat2 in `CapabilityToolMap` — wrong category | `capability_map.rs:40-44` |
| 5 | `cat2_tool_descriptor` unknown-name fallback produces empty descriptor | `tool_registration.rs:200` |
| 6 | No agent-callable tool to list/discover available tools | — |

---

## Confirmed Features to Implement

1. **`ToolRegistryHandle` extended with `lookup_descriptor`** — so the executor can fetch a
   full tool descriptor (name, description, input schema) by name at runtime.

2. **Real `ToolRegistry` wired into `IpcExecutorFactory`** — replace `MockToolRegistry` with
   the real kernel `ToolRegistry` passed at factory construction time.

3. **Cat1 descriptors merged into `current_tool_list()`** — when building the tool list each
   turn, the executor fetches descriptors for all Cat1 tools present in the token's
   `granted_tools` from the registry and appends them alongside Cat2 descriptors.

4. **`sys/tools` Cat2 discovery tool** — always-present tool that the agent can call to list
   available tools by namespace or keyword. Backed by `ToolRegistry::list_all()`. Returns
   name + description + state per tool. Does NOT flood the LLM context automatically.

5. **Remove `llm/*` from `CapabilityToolMap`** — `llm/complete` and siblings are Cat1 service
   tools dispatched via `router.svc → llm.svc`, not Cat2 tools. Removing them stops the
   `dispatch_category2` catch-all stub from being reached.

6. **Remove the `_ =>` fallback in `cat2_tool_descriptor`** — unknown Cat2 tool names are a
   bug; the fallback should panic in debug or return an error, not silently emit an empty
   descriptor.

---

## Files to Change

### Step 1 — Fix `CapabilityToolMap` (`src/types/capability_map.rs`)

Remove the `llm:inference`, `llm:image`, `llm:speech`, `llm:transcription`, `llm:embedding`
entries from the map. These are Cat1 service tools. The map is only for Cat2 gated tools.

**Test**: existing `test_compute_cat2_tools_no_cat1_tools_registered` + update
`test_compute_cat2_tools_individual_agent_tools` to assert `llm/complete` is not in the result.

---

### Step 2 — Fix `cat2_tool_descriptor` fallback (`src/executor/tool_registration.rs`)

Replace the `_ => serde_json::json!({"name": other, "description": "", ...})` arm with:
```rust
_ => {
    tracing::warn!(tool = other, "cat2_tool_descriptor called for unknown tool");
    serde_json::json!({
        "name": other,
        "description": format!("Unknown Cat2 tool: {other}"),
        "input_schema": { "type": "object", "properties": {}, "required": [] }
    })
}
```
And add `sys/tools` to the match so it has a proper descriptor.

**Test**: existing `test_cat2_descriptor_all_tools` — add `"sys/tools"` to the known list.

---

### Step 3 — Extend `ToolRegistryHandle` + implement for real registry (`src/executor/runtime_executor.rs`, `src/tool_registry/registry.rs`)

Add to `ToolRegistryHandle`:
```rust
fn lookup_descriptor(
    &self,
    name: &str,
) -> impl std::future::Future<Output = Option<serde_json::Value>> + Send;
```

Implement for `Arc<MockToolRegistry>`: return `None` always (tests don't need real descriptors).

Add a new concrete type `Arc<ToolRegistry>` implementing `ToolRegistryHandle`:
- `register_tool` / `deregister_tool`: no-ops (ToolRegistry manages its own entries via `ipc.tool-add`)
- `lookup_descriptor`: calls `ToolRegistry::lookup(name).await.ok().map(|e| e.descriptor)`

Update `RegistryRef` enum to add `Real(Arc<ToolRegistry>)` variant alongside `Mock(Arc<MockToolRegistry>)`.

**Test**: unit test that `lookup_descriptor` on a real registry returns the descriptor stored via `add`.

---

### Step 4 — Wire real `ToolRegistry` into `IpcExecutorFactory` (`src/bootstrap/executor_factory.rs`)

Add `tool_registry: Arc<ToolRegistry>` field to `IpcExecutorFactory`. Update `new()` and
constructor callsites in the kernel bootstrap to pass the registry.

In `launch()`, pass `RegistryRef::Real(Arc::clone(&self.tool_registry))` to
`RuntimeExecutor::spawn_with_registry` instead of `MockToolRegistry::new()`.

**Test**: integration test verifying that a spawned executor's registry ref is `Real` variant.

---

### Step 5 — Merge Cat1 descriptors into `current_tool_list()` (`src/executor/tool_manager.rs`, `src/executor/runtime_executor.rs`)

Add to `ToolManager`:
```rust
pub cat1_descriptors: HashMap<String, serde_json::Value>,
```

In `RuntimeExecutor::refresh_tool_list()`, after rebuilding Cat2 descriptors:
1. For each name in `token.granted_tools` that is NOT a Cat2 tool (`!is_cat2_tool(name)`):
2. Call `registry.lookup_descriptor(name).await`
3. If found, convert to LLM tool call format and append to `tool_list`

Store the result in `ToolManager::cat1_descriptors` so it can be refreshed each turn.

**Test**: build an executor with a real registry containing `fs/read` descriptor; assert
`current_tool_list()` includes `fs/read` when the token grants it.

---

### Step 6 — Add `sys/tools` Cat2 tool (`src/executor/tool_registration.rs`, `src/executor/runtime_executor/dispatch_manager.rs`, `src/types/capability_map.rs`)

**Descriptor** in `cat2_tool_descriptor`:
```rust
"sys/tools" => serde_json::json!({
    "name": "sys/tools",
    "description": "List tools available in the Avix runtime. Returns name, description, and state. Use namespace or keyword to filter. Call this to discover tools before requesting access.",
    "input_schema": {
        "type": "object",
        "properties": {
            "namespace": { "type": "string", "description": "Filter by namespace prefix e.g. 'fs', 'llm'" },
            "keyword":   { "type": "string", "description": "Filter by keyword in name or description" },
            "granted_only": { "type": "boolean", "description": "If true, only show tools already in your capability token" }
        },
        "required": []
    }
})
```

Add `"sys/tools"` to `CapabilityToolMap::always` (always-present, no grant required).

**Dispatch** in `dispatch_category2`:
```rust
"sys/tools" => {
    let namespace = call.args["namespace"].as_str().unwrap_or("").to_string();
    let keyword   = call.args["keyword"].as_str().unwrap_or("").to_string();
    let granted_only = call.args["granted_only"].as_bool().unwrap_or(false);

    if let Some(kernel) = &self.kernel {
        let summaries = kernel.list_tools(namespace, keyword, granted_only, &self.token).await;
        return Ok(serde_json::json!({ "tools": summaries }));
    }
    Ok(serde_json::json!({ "tools": [] }))
}
```

Add `list_tools(namespace, keyword, granted_only, token)` to `MockKernelHandle` (backed by
`ToolRegistry::list_all()`), with filtering applied.

**Test**: dispatch `sys/tools` with no filter, assert result contains kernel syscall tools.
Dispatch with `namespace: "fs"`, assert only `fs/*` tools returned.
Dispatch with `granted_only: true`, assert only token-granted tools returned.

---

## Implementation Order

1. `src/types/capability_map.rs` — remove `llm/*` from Cat2 map
2. `src/executor/tool_registration.rs` — fix fallback, add `sys/tools` descriptor
3. `src/executor/runtime_executor.rs` — extend `ToolRegistryHandle`, add `Real` registry variant
4. `src/tool_registry/registry.rs` — implement `ToolRegistryHandle` for `Arc<ToolRegistry>`
5. `src/bootstrap/executor_factory.rs` — wire real registry into `IpcExecutorFactory`
6. `src/executor/tool_manager.rs` — add `cat1_descriptors`, merge into `current_tool_list()`
7. `src/executor/runtime_executor/dispatch_manager.rs` — add `sys/tools` dispatch arm

---

## Testing Strategy

```bash
cargo test --package avix-core types::capability_map
cargo test --package avix-core executor::tool_registration
cargo test --package avix-core executor::runtime_executor
cargo test --package avix-core tool_registry::registry
cargo test --package avix-core bootstrap::executor_factory
cargo test --package avix-core executor::tool_manager
cargo test --package avix-core executor::runtime_executor::dispatch_manager
```

Target: all existing tests pass + new tests for each step above.

---

## Success Criteria

- `cargo clippy --package avix-core -- -D warnings` passes
- `llm/complete` no longer appears in `compute_cat2_tools` output
- An executor with a real registry and token granting `fs/read` has `fs/read` in its
  `current_tool_list()` output when the registry holds a `fs/read` entry
- Agent calling `sys/tools` receives a list of registered tools filtered by the given params
- No `MockToolRegistry` used in `IpcExecutorFactory::launch()`
