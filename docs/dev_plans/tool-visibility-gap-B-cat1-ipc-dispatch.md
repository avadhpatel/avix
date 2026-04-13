# tool-visibility-gap-B — Cat1 IPC Dispatch via Router

**Status**: Complete  
**Last Updated**: 2026-04-13  
**Depends on**: `tool-visibility-gap-A` (registry must be wired into executor first)

---

## Summary

When the LLM calls a Cat1 service tool (e.g. `fs/read`, `llm/complete`), the executor routes
it to `dispatch_via_router` which is currently a stub. No actual IPC call is made. This plan
wires the full dispatch path: executor looks up the tool's `IpcBinding` from `ToolRegistry`,
opens a fresh IPC connection to the service's socket, sends a JSON-RPC request, and returns
the response to the LLM.

This is the last piece needed for agents to actually use Cat1 service tools end-to-end.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` — Cat1 dispatch model
- `docs/architecture/07-services.md` — `IpcBinding`, service socket paths, `_caller` injection
- `crates/avix-core/src/ipc/` — `frame`, `client`, `message` (JSON-RPC 2.0)
- `crates/avix-core/src/tool_registry/descriptor.rs` — `IpcBinding` struct
- `crates/avix-core/src/executor/runtime_executor/dispatch_manager.rs` — `dispatch_via_router`

---

## Architecture Invariants Being Upheld

- **ADR-05**: Fresh IPC connection per call — no persistent channels
- **ADR-03**: Tool names use `/` on the Avix side; wire-mangled `__` only in provider adapters
- **Invariant 4**: ATP = external, IPC = internal — dispatch stays on local sockets
- **Invariant 6**: 4-byte little-endian length-prefix framing over every IPC message
- **Invariant 7**: Long-running tools return `job_id` immediately; progress via `jobs.svc`
- **`_caller` injection**: When `ServiceRegistry::is_caller_scoped(svc)` is true, inject
  `_caller` with agent PID + session ID into the JSON-RPC params

---

## What `dispatch_via_router` Must Do

For a Cat1 tool call the executor must:

1. Look up the tool's `ToolEntry` from the real `ToolRegistry` (available after gap-A)
2. Extract `descriptor.ipc.endpoint` (the service socket path or logical name) and
   `descriptor.ipc.method` (the JSON-RPC method to call)
3. Resolve the endpoint to a socket path (env var override → `runtime_dir/<endpoint>.sock`)
4. Open a fresh `UnixStream` connection (ADR-05)
5. Optionally inject `_caller` if the service is caller-scoped
6. Write a length-prefixed JSON-RPC 2.0 request (`frame::write_to`)
7. Read the length-prefixed response (`frame::read_from`)
8. Return the JSON-RPC `result` field to the executor, or propagate the `error` field

If the tool has no `ipc` binding in its descriptor (kernel syscalls that are dispatched
inline), fall through to the existing kernel syscall dispatch path.

---

## Files to Change

### Step 1 — IPC routing helper (`src/executor/ipc_dispatch.rs` NEW)

Create a new module with a single async function:

```rust
pub async fn dispatch_cat1_tool(
    call: &AvixToolCall,
    entry: &ToolEntry,
    agent_pid: u64,
    session_id: &str,
    runtime_dir: &Path,
    caller_scoped: bool,
) -> Result<serde_json::Value, AvixError>
```

Logic:
1. Extract `IpcBinding` from `entry.descriptor["ipc"]`; if absent return
   `Err(AvixError::ConfigParse("tool has no IPC binding"))`.
2. Resolve socket path:
   - Check `AVIX_<ENDPOINT_UPPER>_SOCK` env var first
   - Fall back to `runtime_dir/<endpoint>.sock`
3. Build JSON-RPC params from `call.args`; if `caller_scoped`, inject:
   ```json
   { "_caller": { "pid": agent_pid, "session_id": session_id } }
   ```
4. Send request via `frame::write_to`, receive via `frame::read_from` (fresh `UnixStream`).
5. On JSON-RPC error field: return `Err(AvixError::Rpc(...))`.
6. Return `response.result`.

**Tests** (unit, using a mock Unix socket listener):
- Successful round-trip: mock listener echoes params back as result, assert returned.
- `_caller` injection: assert injected when `caller_scoped = true`, absent when false.
- Missing IPC binding: assert `ConfigParse` error returned.
- Socket connect failure: assert error propagated cleanly.

---

### Step 2 — Kernel syscall inline dispatch (`src/executor/syscall_dispatch.rs` NEW)

Kernel tools (namespace `kernel/`) have no `IpcBinding` — they're handled by the kernel's
own IPC server, not a service socket. Extract the existing stub body into a proper function:

```rust
pub async fn dispatch_kernel_syscall(
    call: &AvixToolCall,
    kernel: &KernelHandle,
) -> Result<serde_json::Value, AvixError>
```

For now, this sends the call to the kernel IPC server socket (`AVIX_KERNEL_SOCK` /
`runtime_dir/kernel.sock`) using the same `dispatch_cat1_tool` logic with `_caller` always
injected.

**Tests**: assert kernel namespace tools route to kernel socket path.

---

### Step 3 — Wire `dispatch_via_router` (`src/executor/runtime_executor/dispatch_manager.rs`)

Replace the stub body:

```rust
pub async fn dispatch_via_router(
    &self,
    call: &AvixToolCall,
) -> Result<serde_json::Value, AvixError> {
    // 1. Look up the tool entry in the real registry
    let entry = self.registry_ref.lookup_descriptor(&call.name).await
        .ok_or_else(|| AvixError::ConfigParse(
            format!("tool '{}' not found in registry", call.name)
        ))?;

    // 2. Check if it's a kernel syscall (no IPC binding → kernel socket)
    if entry["ipc"].is_null() {
        return dispatch_kernel_syscall(call, &self.runtime_dir).await;
    }

    // 3. Resolve caller-scoped flag from ServiceRegistry
    let caller_scoped = is_caller_scoped_tool(&call.name);

    // 4. Dispatch via IPC
    dispatch_cat1_tool(
        call,
        &entry,
        self.pid.as_u64(),
        &self.session_id,
        &self.runtime_dir,
        caller_scoped,
    )
    .await
}
```

**Tests**:
- Dispatch a tool whose registry entry has an `IpcBinding` pointing to a mock socket;
  assert the result comes back correctly.
- Dispatch a tool not in the registry; assert a clean error (not a panic or stub response).

---

### Step 4 — Permission check before dispatch (`src/executor/runtime_executor/dispatch_manager.rs`)

Before calling `dispatch_via_router` (and `dispatch_category2`), verify the calling agent has
execute (`x`) permission:

```rust
fn check_tool_execute_permission(
    entry: &ToolEntry,
    username: &str,
    role: &str,
) -> Result<(), AvixError>
```

Logic (matching the design from TODO.md Phase 5):
- `role == "admin"` → allow
- `username == entry.permissions.owner` → check owner rwx contains `x`
- else → check `entry.permissions.all` contains `x`

If denied: return `Err(AvixError::CapabilityDenied(...))`.

Wire into `dispatch_via_router` after registry lookup, before IPC call.
**Not** wired into `dispatch_category2` yet — Cat2 tools are always executor-internal and
permission-checked implicitly via the capability token.

**Tests**:
- Owner with `rwx` → allowed
- Non-owner with `all: "r--"` → denied
- Admin role → always allowed

---

### Step 5 — Expose `is_caller_scoped` to executor (`src/service/registry.rs` or inline)

Add a lightweight helper that returns true for tool namespaces whose service is caller-scoped
(i.e. the service has `caller_scoped: true` in its `ServiceRecord`). For now this can be a
simple hardcoded list or a lookup against `ServiceManager` if available via the kernel handle.

---

## Implementation Order

1. `src/executor/ipc_dispatch.rs` (NEW) — `dispatch_cat1_tool` with tests
2. `src/executor/syscall_dispatch.rs` (NEW) — `dispatch_kernel_syscall` with tests
3. `src/executor/runtime_executor/dispatch_manager.rs` — wire `dispatch_via_router`
4. `src/executor/runtime_executor/dispatch_manager.rs` — add permission check
5. `src/executor/mod.rs` — expose new modules

---

## Testing Strategy

```bash
cargo test --package avix-core executor::ipc_dispatch
cargo test --package avix-core executor::syscall_dispatch
cargo test --package avix-core executor::runtime_executor::dispatch_manager
```

Integration test using a real `UnixListener` in `crates/avix-core/tests/`:
- Start a mock service that accepts `fs/read` calls and returns `{"content": "hello"}`
- Spawn an executor with the registry wired (from gap-A)
- Run `run_with_client` with an LLM mock that calls `fs/read`
- Assert the executor returns the mock service's response, not the stub

---

## Success Criteria

- `dispatch_via_router` no longer returns the `"IPC dispatch not yet wired"` placeholder
- A Cat1 tool call with a valid `IpcBinding` is forwarded over a Unix socket and the
  response is returned to the LLM
- `_caller` is injected only for caller-scoped services
- Agents lacking execute permission on a tool receive `CapabilityDenied`, not a stub result
- `cargo clippy --package avix-core -- -D warnings` passes

---

## Dependencies

- **gap-A must be complete first**: `dispatch_via_router` relies on `registry_ref.lookup_descriptor()`
  which is wired in gap-A Step 3.
- `IpcBinding` is already defined in `src/tool_registry/descriptor.rs`.
- `frame::write_to` / `frame::read_from` already exist in `src/ipc/frame.rs`.
- `UnixStream` already used in `IpcLlmClient` — same pattern.
