# IPC Gap B — Router Dispatch & Tool Call Enforcement

> **Status:** Not started
> **Priority:** Critical — depends on Gap A (transport server)
> **Affects:** `avix-core/src/router/`, `avix-core/src/executor/runtime_executor.rs`

---

## Problem

`router/registry.rs` maps tool names → service endpoints and `router/concurrency.rs` enforces limits, but there is no actual dispatch path that:
1. Accepts an incoming tool-call request
2. Resolves the owning service endpoint from the registry
3. Applies concurrency limits (backpressure)
4. Injects `_caller` into params
5. Forwards to the service via IPC client
6. Returns the service response (or error) to the caller

Additionally, tool name mangling (`/` → `__` on the wire to providers) is mentioned in the architecture (ADR-03) but not implemented anywhere.

`RuntimeExecutor` has no real tool invocation path — it registers tools but never dispatches calls through IPC.

---

## What Needs to Be Built

### 1. Router Dispatcher (`router/dispatcher.rs`)

```rust
pub struct RouterDispatcher {
    registry: Arc<RwLock<ServiceRegistry>>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    concurrency: Arc<ConcurrencyLimiter>,
    caller_limits: Arc<RwLock<CallerScopedLimiter>>,
    run_dir: PathBuf,
}

impl RouterDispatcher {
    pub fn new(
        registry: Arc<RwLock<ServiceRegistry>>,
        tool_registry: Arc<RwLock<ToolRegistry>>,
        run_dir: PathBuf,
    ) -> Self;

    /// Dispatch a tool call from a known caller.
    /// Enforces concurrency limits, injects _caller, forwards to service.
    pub async fn dispatch(
        &self,
        request: JsonRpcRequest,
        caller_pid: Pid,
        caller_user: &str,
        caller_token: &str,
    ) -> JsonRpcResponse;
}
```

Internal flow for `dispatch()`:

```
1. Strip tool name from request.method
2. Look up tool in ToolRegistry → get owning service name
   - If not found: return ENOTFOUND_METHOD
   - If state is Unavailable: return EUNAVAIL
3. Acquire ToolCallGuard (in-flight semaphore for drain support)
4. Acquire ConcurrencyLimiter slot
   - If at capacity: return EBUSY
5. Resolve service endpoint path from ServiceRegistry
   - If service not found: return EUNAVAIL
6. Inject _caller into request.params
7. Open IpcClient to service endpoint path
8. Forward request, await response
9. Release concurrency slot (via drop)
10. Return response
```

Timeout: wrap step 8 with `tokio::time::timeout(service_queue_timeout)`. Return `ETIMEOUT` on expiry.

### 2. Tool Name Mangler (`router/mangle.rs`)

Per ADR-03 and §11 of CLAUDE.md:

```rust
/// Replace `/` with `__` for wire format (e.g. "fs/read" → "fs__read")
pub fn mangle(name: &str) -> String;

/// Replace `__` with `/` when receiving from provider (e.g. "fs__read" → "fs/read")
pub fn unmangle(name: &str) -> String;

/// Return Err if name contains `__` (reserved for wire only)
pub fn validate_tool_name(name: &str) -> Result<(), AvixError>;
```

Invariant: `ToolName::parse` (already exists) must reject names with `__`. These functions are used only at the provider adapter boundary — nowhere else in the codebase should `__` appear.

### 3. Capability Enforcement (`router/capability.rs`)

Before dispatching, verify the caller has the tool in its `granted_tools`:

```rust
/// Returns Ok(()) if the calling process has the named tool in its CapabilityToken.
/// Returns Err(AvixError::Eperm) if not granted.
pub async fn check_capability(
    tool: &str,
    caller_pid: Pid,
    process_table: &Arc<RwLock<ProcessTable>>,
) -> Result<(), AvixError>;
```

- Reads `ProcessEntry.granted_tools` from the process table
- Checks if `tool` is in the list
- Always allows `cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch` (always-present per ADR-04)

### 4. Router IPC Server Entry Point (`router/server.rs`)

Wraps `IpcServer` (from Gap A) with `RouterDispatcher`:

```rust
pub struct RouterServer {
    dispatcher: Arc<RouterDispatcher>,
    server: IpcServer,
}

impl RouterServer {
    pub async fn bind(dispatcher: Arc<RouterDispatcher>, run_dir: &Path) -> Result<Self, AvixError>;
    pub async fn serve(self) -> Result<(), AvixError>;
}
```

The handler passed to `IpcServer::serve`:
1. Extracts `_caller` from params if present (for service-to-service calls)
2. Falls back to a kernel service identity for calls without explicit `_caller`
3. Calls `dispatcher.dispatch(...)`

### 5. RuntimeExecutor Real Dispatch

Currently `RuntimeExecutor` has no tool invocation path. After tool calls are returned by the LLM, they must be dispatched through the router. Add:

```rust
impl RuntimeExecutor {
    /// Dispatch a single tool call decided by the LLM.
    /// Checks HIL requirements, chain depth, budgets, then calls dispatcher.
    pub async fn invoke_tool(
        &self,
        name: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AvixError>;
}
```

Internal flow:
1. Check `hil_required_tools` — if tool requires HIL, pause (signal bus) and wait
2. Check `tool_budgets` — if budget exhausted, return `ELIMIT`
3. Increment process table chain depth
4. Build `JsonRpcRequest` with the tool name and params
5. Call `RouterDispatcher::dispatch` with own PID/user/token
6. Decrement chain depth on return (drop guard)
7. Return result or propagate error

---

## TDD Test Plan

All tests go in `crates/avix-core/tests/router_dispatch.rs`.

```rust
// T-B-01: Successful tool dispatch round-trip
#[tokio::test]
async fn dispatch_routes_to_correct_service() {
    // register "echo-svc" with tool "echo/ping"
    // start a mock IpcServer as "echo-svc" that returns {pong: true}
    // dispatch("echo/ping") from pid=10
    // assert result is {pong: true}
}

// T-B-02: Unknown tool returns ENOTFOUND_METHOD
#[tokio::test]
async fn dispatch_unknown_tool_returns_not_found() {
    // dispatch("ghost/unknown")
    // assert error code -32601
}

// T-B-03: Unavailable tool returns EUNAVAIL
#[tokio::test]
async fn dispatch_unavailable_tool_returns_eunavail() {
    // register tool, set state to Unavailable
    // dispatch
    // assert error code -32005
}

// T-B-04: Concurrency limit respected
#[tokio::test]
async fn dispatch_at_capacity_returns_ebusy() {
    // set max_concurrent = 1
    // hold one in-flight call open
    // second dispatch returns EBUSY
}

// T-B-05: Timeout returns ETIMEOUT
#[tokio::test]
async fn dispatch_slow_service_returns_etimeout() {
    // service handler sleeps 200ms
    // dispatcher timeout = 50ms
    // assert ETIMEOUT error code
}

// T-B-06: _caller is injected into forwarded params
#[tokio::test]
async fn dispatch_injects_caller() {
    // inspect what params the mock service receives
    // assert _caller.pid = caller_pid, _caller.user = caller_user
}

// T-B-07: Tool name mangle/unmangle round-trip
#[test]
fn mangle_unmangle_round_trip() {
    assert_eq!(mangle("fs/read"), "fs__read");
    assert_eq!(unmangle("fs__read"), "fs/read");
    assert!(validate_tool_name("fs__read").is_err());
    assert!(validate_tool_name("fs/read").is_ok());
}

// T-B-08: Capability check blocks unauthorized tool
#[tokio::test]
async fn capability_check_blocks_unauthorized() {
    // process table has pid=10 with granted_tools=["fs/read"]
    // check_capability("fs/write", 10) → Err(Eperm)
    // check_capability("fs/read", 10) → Ok(())
}

// T-B-09: Always-present tools bypass capability check
#[tokio::test]
async fn always_present_tools_are_always_allowed() {
    // process with empty granted_tools
    // check_capability("cap/request-tool") → Ok(())
    // check_capability("job/watch") → Ok(())
}

// T-B-10: invoke_tool respects budget
#[tokio::test]
async fn invoke_tool_budget_exhausted_returns_elimit() {
    // executor with budget("fs/read", 2)
    // call fs/read 3 times
    // third call returns ELIMIT
}
```

---

## Implementation Notes

- `RouterDispatcher::dispatch` must NOT hold `RwLock` guards across `.await` points — take short read locks, copy needed data, drop guard, then await
- Tool call guard (`ToolCallGuard` from `ToolRegistry::acquire`) must be held for the duration of the forwarded call to enable drain support
- Do not implement cross-process capability token verification now — check only `ProcessTable.granted_tools` (set at spawn by RuntimeExecutor)
- Always-present tool list: `["cap/request-tool", "cap/escalate", "cap/list", "job/watch"]` — define as constant in `capability.rs`

---

## Success Criteria

- [ ] All T-B-* tests pass
- [ ] Tool mangle/unmangle functions implemented and tested
- [ ] Capability enforcement wired into dispatch path
- [ ] `invoke_tool` in `RuntimeExecutor` dispatches through `RouterDispatcher`
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes (no regressions)
