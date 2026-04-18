# Tool Registry Unification - Remaining Items

**Status**: Phase 5 partial — tool visibility complete, VFS permission wiring pending  
**Last Updated**: 2026-04-18

---

## Completed Work

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | ✅ Done | Kernel Syscall Registry - `SyscallDescriptor` + `SyscallRegistry` with 26 syscalls |
| Phase 2 | ✅ Done | ToolRegistry Unification - kernel syscalls in registry, capabilities_required field |
| Phase 3 | ✅ Done | /tools VFS Mount - lazy population, tool YAML descriptors |
| gap-A | ✅ Done | Registry wiring & discovery — real `ToolRegistry` wired into `IpcExecutorFactory`, Cat1 descriptors merged into `current_tool_list()`, `sys/tools` Cat2 discovery tool, `llm/*` removed from `CapabilityToolMap` ([plan](tool-visibility-gap-A-registry-wiring-and-discovery.md)) |
| gap-B | ✅ Done | Cat1 IPC dispatch — `dispatch_via_router` fully wired, `dispatch_cat1_tool` + `dispatch_kernel_syscall` over fresh Unix sockets, execute permission check ([plan](tool-visibility-gap-B-cat1-ipc-dispatch.md)) |
| Phase 4a | ✅ Done | `ToolPermissions` struct with owner/crew/all fields — implemented as part of gap-B permission check |
| Phase 4b | ✅ Done | `ToolPermissions` wired from `tool.yaml` into `ToolEntry` via `ToolScanner` ([plan](tool-visibility-per-agent-state.md)) |
| Phase 4c | ✅ Done | Per-agent tool state: `init_vfs_caller()` sets `VfsCallerContext` on VFS; `IpcExecutorFactory` wires VFS + calls `init_vfs_caller` at spawn; `agent.spawned` event emitted (commit `accb915`) |

---

## Remaining Items (Phase 5+)

### 0. Cat1 Tool Pipeline — Remaining Work

Items deferred from `cat1-tool-pipeline-fix.md` (commit `4a126ba`). These are the highest
priority items — agents cannot do meaningful work without `fs/*` and `exec/*` tools.

#### 0a. Token resolution from manifest `requestedCapabilities`

The `CapabilityToken` in `proc/agent.rs` is still hardcoded with 3 tools (`agent/spawn`,
`llm/complete`, `llm/embed`). It must be resolved from the agent manifest's
`requestedCapabilities` field so each agent gets exactly the tools its manifest declares.

**Requires**: a resolver that maps capability group strings (e.g. `fs:*`, `llm:inference`,
`kernel:*`) to individual tool names by querying the tool registry and syscall registry.

**Affected files**: `kernel/proc/agent.rs`, new `kernel/capability_resolver.rs`.

#### 0b. `fs.svc` — VFS Cat1 tools (`fs/read`, `fs/write`, `fs/list`, `fs/exists`, `fs/delete`)

No filesystem service exists. Agents cannot read or write files. The VFS is operational
internally but not exposed as Cat1 tools to the LLM.

**Options** (decide at plan time):
1. Add a lightweight `fs.svc` IPC server that wraps `VfsRouter` calls.
2. Route `fs/*` calls through the kernel IPC server (already has `VfsRouter` access).

Option 2 is simpler — add `fs/*` handlers directly to `KernelIpcServer` and register the
tools with no IPC binding (routes to `kernel.sock` automatically). The kernel IPC already has
`VfsRouter` access through `ProcHandler`.

**Affected files**: `kernel/ipc_server.rs`, `bootstrap/mod.rs` (register `fs/*` tools).

#### 0c. `kernel/fs/*` kernel IPC handlers

`SyscallRegistry` defines `kernel/fs/read`, `kernel/fs/write`, etc. but `KernelIpcServer`
does not handle them. These need handlers that delegate to `VfsRouter`.

Closely related to 0b — implement together.

#### 0d. `exec.svc` Cat1 tools (`exec/python`, `exec/shell`, etc.)

`exec.svc` IPC server is running (`exec.sock` is created) but its tools are not registered in
the tool registry. Follow the same pattern as `llm.svc` registration added in `4a126ba`.

**Affected files**: `bootstrap/mod.rs` (register `exec/*` tools after `exec.svc` starts).

---

### 1. Linux-Style Permission Model (rwx)

**Goal**: Implement owner/crew/all permissions like Linux file system.

**Required Changes**:

| File | Change |
|------|--------|
| `src/tool_registry/permissions.rs` (NEW) | Define `ToolPermissions` struct with owner/crew/all fields (each rwx) |
| `src/tool_registry/entry.rs` | Add `permissions: ToolPermissions` field to `ToolEntry` |
| `src/tool_registry/scanner.rs` | Parse optional `permissions` from tool.yaml |
| `src/memfs/router.rs` | Check permissions in `generate_tool_yaml()` |

**Design**:
```rust
// ToolPermissions in src/tool_registry/permissions.rs
pub struct ToolPermissions {
    pub owner: String,      // user who owns the tool
    pub crew: String,      // crew name (optional)
    pub all: String,       // "r--", "rw-", "rwx" for everyone
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            owner: "admin".to_string(),
            crew: "".to_string(),
            all: "r--".to_string(),  // Default: read-only for all
        }
    }
}
```

**Permission Check Logic**:
```
Input: caller (username, role, crews[]), tool_permissions

if role == "admin" → return "rwx" (full access)
if caller.username == tool_permissions.owner → return owner perms
if caller.crews contains tool_permissions.crew → return crew perms
return tool_permissions.all perms
```

---

### 2. Per-Agent Tool State (available vs unavailable) — ✅ RESOLVED

**Commit**: `accb915` (2026-04-17). See [tool-visibility-per-agent-state.md](tool-visibility-per-agent-state.md).

- `RuntimeExecutor::init_vfs_caller()` builds `VfsCallerContext` from agent token + `spawned_by` and calls `vfs.set_caller()`
- `IpcExecutorFactory` now holds `vfs: Option<Arc<VfsRouter>>`, wires it via `with_vfs()` at spawn, and calls `init_vfs_caller()` 
- `generate_tool_yaml()` already had the per-agent logic; it now executes because caller is no longer `None`
- Unavailable tools show `state: unavailable` + `request_access: cap/request-tool`

---

### 3. HIL Path for Requesting Access — ✅ RESOLVED

Already emitted in `generate_tool_yaml()` as part of the per-agent tool state fix above.

---

## Implementation Order

1. **Phase 4a**: ✅ Implement `ToolPermissions` struct + default to all r--
2. **Phase 4b**: ✅ Update scanner to read permissions from tool.yaml
3. **Phase 4c**: ✅ Per-agent tool state via `init_vfs_caller()` (commit `accb915`)
4. **Cat1 0d**: Register `exec.svc` tools in tool registry (quick, follows llm.svc pattern)
5. **Cat1 0b+0c**: `fs/*` Cat1 tools — kernel IPC handlers + registry registration
6. **Cat1 0a**: Token resolution from manifest `requestedCapabilities`
7. **Phase 5a**: Linux-style rwx permission enforcement in `generate_tool_yaml()`
8. **Phase 5b**: Wire crew membership into `VfsCallerContext` at spawn
9. **Phase 5c**: Enforce write/execute permissions at tool dispatch time

---

## Testing Strategy

```bash
# Run specific tests
cargo test --package avix-core tool_registry::permissions
cargo test --package avix-core tool_registry::state
cargo test --package avix-core memfs::router

# Full test
cargo test --package avix-core --lib
```

---

---

## Packaging & Installation — Future Work

### Remote Binary Upload for Local Installs over ATP

**Context**: `proc/package/install-agent` and `proc/package/install-service` accept a source
string (URL, `github:` spec, or server-side file path). All download/extract work happens
server-side. This means `file:///path` installs only work when the client and server share a
filesystem (local dev). A remote client (e.g. Web-UI on a laptop, server on a remote box)
cannot install from a local `.tar.xz` file it holds.

**Goal**: Allow a client to push a local `.tar.xz` binary directly to the kernel over ATP,
so remote installs from local files work.

**Options to evaluate**:
1. **Chunked ATP upload** — new `proc/package/upload-chunk` ATP command that accepts
   base64-encoded byte chunks + a final `proc/package/install-from-upload` that assembles
   and installs. Simple but slow for large service binaries.
2. **Separate HTTP upload endpoint** — `POST /api/v1/package/upload` (multipart) on the
   ATP gateway's HTTP layer. Returns a temp token; `proc/package/install-agent` accepts
   `upload:<token>` as a source. Faster, standard pattern, fits well with the existing
   HTTP login endpoint.

Option 2 is preferred — the HTTP layer already exists for `/auth/login`.

**Affected files** (when implemented):
- `crates/avix-core/src/gateway/` — add multipart upload handler
- `crates/avix-core/src/syscall/domain/pkg_.rs` — handle `upload:<token>` source
- CLI — add `--file <path>` flag to `avix agent install` / `avix service install`
- Web-UI — file picker in Extensions tab "Install URL" form

---

### Polyglot Services (Python, JavaScript, etc.)

**Context**: The current service model assumes Rust binaries. `service.yaml` has a `language`
field that accepts `"rust"` or `"any"`, and `ServiceInstaller` / `ServiceProcess` both assume
a compiled native binary in `bin/`. There is no mechanism to spawn or package services written
in Python, JavaScript/TypeScript, or other interpreted languages.

**Goal**: Define how non-Rust services are packaged, installed, and spawned so that community
developers can write Avix services in their language of choice.

**Questions to resolve**:

1. **Runtime discovery** — how does the kernel find the right interpreter (`python3`, `node`,
   `deno`, etc.)? Options: require it on `PATH`, embed a runtime version spec in `service.yaml`,
   or bundle the runtime inside the package.

2. **Packaging** — for a Python service the `bin/` dir would contain `.py` files (or a wheel),
   not a compiled binary. The `PackageBuilder` and `PackageValidator` need to know what
   constitutes a valid `bin/` for each language.

3. **`service.yaml` changes** — `language` field needs well-defined values beyond `"rust"` /
   `"any"`. Proposed additions: `"python"`, `"javascript"`, `"typescript"`, `"deno"`.
   May also need `runtime_version` (e.g. `">=3.11"`) and `entrypoint` (e.g. `"main.py"`).

4. **`ServiceProcess` spawning** — currently calls the binary path directly. For interpreted
   languages it needs to prepend the interpreter: `python3 bin/main.py` or `node bin/index.js`.

5. **Dependency management** — Python services may have a `requirements.txt`; JS services a
   `package.json`. Should the installer run `pip install` / `npm install` at install time?
   Or should packages be self-contained (vendored dependencies)?

6. **Sandboxing** — interpreted runtimes have different sandboxing considerations than native
   binaries. Needs evaluation.

**Proposed `service.yaml` sketch**:
```yaml
name: my-python-svc
version: "0.1.0"

[service]
language       = "python"
runtime_version = ">=3.11"
entrypoint     = "bin/main.py"
binary         = ""              # empty for interpreted services
```

**Affected areas** (when designed and implemented):
- `service.yaml` schema + parser (`crates/avix-core/src/service/`)
- `ServiceProcess::spawn` — interpreter prefix logic
- `PackageValidator` — language-specific `bin/` validation rules
- `PackageBuilder` — skip executable permission setting for non-native bins
- `ServiceInstaller` — optional dependency installation step
- `avix package new --type service --language python` scaffold

---

---

## Signal Delivery to Active RuntimeExecutor Threads

**Goal**: When a signal (e.g. `SIGPAUSE`, `SIGKILL`, `SIGSTOP`, `SIGPIPE`) arrives for an
agent that is currently blocked inside an LLM call (`llm/complete` via IPC), the signal must
be delivered promptly and cause the correct observable effect — not silently queued until the
LLM call returns.

**Problem today — two distinct bugs**:

1. **Wrong delivery path (architecture bug)**: The current production code assumes each active
   agent PID has its own dedicated socket for receiving signals. This is incorrect — there is
   no per-agent socket. The kernel delivers signals to `RuntimeExecutor` via the existing
   `deliver_signal` method (called from `ProcHandler` / `KernelIpcServer` on the kernel side).
   Any code that opens or listens on a per-agent signal socket must be removed; signal receipt
   must go through `deliver_signal` exclusively.

2. **Late delivery (timing bug)**: Even once signals arrive via `deliver_signal`, the current
   `RuntimeExecutor` only checks for them between turns (i.e. after the LLM response arrives).
   An in-flight `llm/complete` call can take seconds to minutes, so signals sent during that
   window are not acted on until the call completes — making `SIGKILL`/`SIGPAUSE` feel
   unresponsive and breaking any caller expecting prompt acknowledgement.

**Required design**:

1. **Cancellable LLM future** — wrap the `llm/complete` IPC call in a `tokio::select!` that
   races against a `CancellationToken` (from the `tokio-util` crate).  The token is held by
   `RuntimeExecutor` and cancelled immediately when a `SIGKILL`, `SIGSTOP`, or `SIGPAUSE`
   arrives on the signal channel.

2. **Signal-dispatch loop runs concurrently** — promote the signal-receive loop from
   post-turn polling to a background `tokio::select!` arm that is always live, even during
   the LLM call.  Something like:
   ```rust
   tokio::select! {
       result = llm_call_future => { /* handle LLM response */ }
       sig    = signal_rx.recv() => { handle_signal_during_llm(sig, cancel.clone()); }
   }
   ```

3. **Per-signal semantics during an active call**:
   | Signal       | Action while LLM call is in-flight |
   |--------------|-------------------------------------|
   | `SIGKILL`    | Cancel LLM future immediately; finalize invocation as `Killed`; exit |
   | `SIGSTOP`    | Cancel future; suspend task (do not resume until `SIGSTART`) |
   | `SIGPAUSE`   | Cancel future; enter paused state; resume with `SIGRESUME` |
   | `SIGPIPE`    | Deliver pipe data into the next-turn context; do NOT cancel the current call |
   | `SIGSAVE`    | Flush conversation snapshot mid-call; continue |
   | `SIGESCALATE`| Inject HIL approval result into context; continue or cancel based on result |

4. **State machine update** — `RuntimeExecutor`'s internal state machine must have an
   explicit `ActiveLlmCall { cancel: CancellationToken }` variant so that the signal handler
   can distinguish "idle between turns" from "blocked in LLM call" and apply the right action.

5. **IPC acknowledgement** — after cancelling the LLM future, `RuntimeExecutor` must still
   send the signal acknowledgement back to the kernel (update `/proc/<pid>/status.yaml` and
   emit the appropriate ATP event) before entering the new state.

**Affected files** (to be detailed in the dev plan):
- `crates/avix-core/src/runtime/executor.rs` — `tokio::select!` + `CancellationToken`
- `crates/avix-core/src/runtime/state.rs` (or inline) — add `ActiveLlmCall` state variant
- `crates/avix-core/src/runtime/signal.rs` — signal handler logic split into
  `handle_signal_between_turns` vs `handle_signal_during_llm`
- Integration test in `crates/avix-core/tests/lifecycle.rs` — assert `SIGKILL` while LLM
  call is pending resolves within e.g. 200 ms

**Dependencies**: `tokio-util` crate (already likely present); no new external deps expected.

---

## Session Management — ✅ RESOLVED

All session management commands implemented and committed (2026-04-17/18).
`avix client session list/show/resume/delete` fully wired with ownership enforcement.
See `docs/architecture/06-agents.md` § Session Management and `docs/architecture/12-avix-clients.md`.

---

## Notes

- Permission model defaults to `all: r--` (everyone can read but not execute)
- Admin role gets full rwx on all tools
- VFS needs caller context to compute per-agent state - this may require changes to how VFS resolves the calling agent's identity
- HIL path uses existing `cap/request-tool` - just need to reference it in YAML

---

## Streaming Events Pipeline — ✅ Client-Side Complete / ⏳ Server-Side Pending

Client-side streaming gaps fixed in commit `c7a9dbf` (2026-04-17).
Server-side routing fix tracked in [`streaming-events-gap-D-session-id-routing.md`](streaming-events-gap-D-session-id-routing.md).

| Gap | Description | Status |
|-----|-------------|--------|
| Gap A | `ConnectionStatus` stored hardcoded `"core-init"` session_id | ✅ Fixed (`c7a9dbf`) |
| Gap B | `pid` type mismatch — `u64` vs string in typed body structs | ✅ Fixed (`c7a9dbf`) |
| Gap C | `EventBody::AgentSpawned` variant missing; `AgentSpawnedBody` not defined | ✅ Fixed (`c7a9dbf`) |
| Gap D | `start_event_bridge()` could be double-started on reconnect | ✅ Fixed (`c7a9dbf`) |
| Gap E (server) | `IpcExecutorFactory` passes agent session UUID to `event_bus.*` calls that expect ATP connection session ID — ownership gate always fails, all events dropped | ✅ Fixed (`ef603f8`) |
| Gap F (server) | `agent.spawned` event never emitted by `IpcExecutorFactory` (only in test stubs) | ✅ Fixed (`ef603f8`) |
| Gap G (server) | `agent.tool_call`/`agent.tool_result` used `self.session_id` instead of `self.atp_session_id` | ✅ Fixed (`b350963` + one-liner in dispatch_manager.rs) |

**Streaming pipeline fully operational** — all ownership-scoped events now route correctly.

---

## Agent Tool Visibility — ✅ RESOLVED (gap-A + gap-B)

All items below were fixed as part of gap-A and gap-B (2026-04-12/13).

| # | Bug | Fix | Commit |
|---|-----|-----|--------|
| 1 | `llm/*` incorrectly classified as Cat2 | Removed from `CapabilityToolMap` | gap-A |
| 2 | `cat2_tool_descriptor` silent empty fallback | Replaced with `tracing::warn!` + descriptive message | gap-A |
| 3 | Cat1 descriptors never in `current_tool_list()` | `refresh_tool_list` now fetches from real `ToolRegistry` + merges | gap-A |
| 4 | `RuntimeExecutor` used `MockToolRegistry` in production | Real registry wired via deferred `Arc<Mutex<Option<Arc<ToolRegistry>>>>` injection | gap-A |
| 5 | `dispatch_via_router` stub | Full IPC dispatch over Unix socket; permission check; kernel routing | gap-B |
| 6 | No tool discovery for agents | `sys/tools` Cat2 tool added (always-present) | gap-A |