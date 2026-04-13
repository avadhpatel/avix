# Tool Registry Unification - Remaining Items

**Status**: Phase 3 complete, Phase 4+ pending  
**Last Updated**: 2026-04-01

---

## Completed Work

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | ✅ Done | Kernel Syscall Registry - `SyscallDescriptor` + `SyscallRegistry` with 26 syscalls |
| Phase 2 | ✅ Done | ToolRegistry Unification - kernel syscalls in registry, capabilities_required field |
| Phase 3 | ✅ Done | /tools VFS Mount - lazy population, tool YAML descriptors |

---

## Remaining Items (Phase 4+)

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

### 2. Per-Agent Tool State (available vs unavailable)

**Goal**: Show `state: unavailable` for tools the agent doesn't have capability for.

**Required Changes**:

| File | Change |
|------|--------|
| `src/tool_registry/state.rs` (NEW) | Define `ToolAccessState` enum + method to compute from token |
| `src/memfs/router.rs` | Pass agent context to `generate_tool_yaml()` to compute per-agent state |

**Design**:
```rust
// In src/tool_registry/state.rs
pub enum ToolAccessState {
    Available,    // Agent has the required capability
    Unavailable,  // Agent does NOT have the required capability
}

impl ToolEntry {
    pub fn access_state(&self, token: &CapabilityToken) -> ToolAccessState {
        // Check if all required capabilities are in token
        if self.capabilities_required.iter().all(|c| token.has_capability(c)) {
            ToolAccessState::Available
        } else {
            ToolAccessState::Unavailable
        }
    }
}
```

**VFS Changes**:
- Need to pass agent's username/role/crews to VFS read context
- Tool YAML should show `state: unavailable` + `capabilities_required` when access denied
- This requires changes to how VFS resolves caller identity (probably from ATP token)

---

### 3. HIL Path for Requesting Access

**Goal**: When agent sees unavailable tool, it can request access via `cap/request-tool`.

**Required Changes**:

| File | Change |
|------|--------|
| `src/vfs/tools_provider.rs` or `src/memfs/router.rs` | Add `request_access: cap/request-tool` to unavailable tool YAML |

**YAML Output for Unavailable Tool**:
```yaml
name: kernel/proc/kill
description: Terminate an agent process
capabilities_required:
  - agent:kill
state: unavailable
owner: kernel
request_access: cap/request-tool
# Agent figures out the right reason to request
```

---

## Implementation Order

1. **Phase 4a**: Implement `ToolPermissions` struct + default to all r--
2. **Phase 4b**: Update scanner to read permissions from tool.yaml
3. **Phase 4c**: Add permissions to VFS output
4. **Phase 5a**: Implement per-agent access state from CapabilityToken
5. **Phase 5b**: Wire agent context into VFS reads
6. **Phase 5c**: Add HIL path reference to unavailable tools

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

## Session Management

### Session Delete (`avix session delete <id>`)

**Plan**: [session-gap-B-session-delete.md](session-gap-B-session-delete.md)

`SessionStore::delete()` exists at the store layer but is not wired to `PersistentSessionStore`,
the IPC server (`kernel/proc/session/delete`), the ATP gateway, or `avix session delete <id>` CLI.
Workaround: stop kernel, delete `<root>/data/sessions.redb`.

---

## Notes

- Permission model defaults to `all: r--` (everyone can read but not execute)
- Admin role gets full rwx on all tools
- VFS needs caller context to compute per-agent state - this may require changes to how VFS resolves the calling agent's identity
- HIL path uses existing `cap/request-tool` - just need to reference it in YAML

---

## Agent Tool Visibility — Cat1 Tools Not Sent to LLM

**Problem**: The LLM only receives tool descriptors for Cat2 tools (`agent/`, `pipe/`, `cap/`,
`job/`). Cat1 service tools (`fs/read`, `fs/write`, etc.) are granted in the capability token
but never appear in the tool list sent to Claude each turn. The agent cannot use them — the LLM
doesn't know they exist as callable tools.

**Root cause**: `current_tool_list()` is built solely from `compute_cat2_tools()`. Cat1 tool
descriptors are never fetched or stored by `RuntimeExecutor`.

**Related bugs discovered during investigation**:

1. **`llm/*` tools incorrectly classified as Cat2** (`src/types/capability_map.rs:40`)  
   `llm/complete`, `llm/generate-image`, etc. are listed in `CapabilityToolMap` under
   `"llm:inference"` etc., making `is_cat2_tool("llm/complete")` return `true` at runtime.
   They should be Cat1 service tools dispatched via `router.svc → llm.svc`. Consequence:
   `dispatch_category2` is called but has no handler for them — hits the `_` catch-all stub.

2. **`llm/complete` gets an empty descriptor** (`src/executor/tool_registration.rs`)  
   `cat2_tool_descriptor("llm/complete")` falls to the `_ =>` arm and returns `description: ""`
   with an empty schema. The LLM sees a useless tool definition and calls it blindly.

3. **`ipc.tool-add` discards the full descriptor** (`src/executor/tool_manager.rs`)  
   When a service registers a tool via `ipc.tool-add`, only the tool name is tracked in
   `registered_cat2`. The full JSON schema descriptor is discarded. There is nowhere to store
   Cat1 tool descriptors received at runtime.

4. **`dispatch_via_router` is a stub** (`dispatch_manager.rs:278`)  
   Cat1 tool calls route to `dispatch_via_router`, which returns a placeholder. No IPC to
   `router.svc` is wired.

**Required fixes** (in implementation order):

| # | File | Change |
|---|------|--------|
| 1 | `src/types/capability_map.rs` | Remove `llm/*` entries — they are Cat1 service tools, not Cat2 |
| 2 | `src/executor/tool_manager.rs` | Add `cat1_descriptors: HashMap<String, serde_json::Value>` field; store full descriptor on `ipc.tool-add` |
| 3 | `src/executor/runtime_executor.rs` / `dispatch_manager.rs` | In `current_tool_list()`, merge Cat2 descriptors + Cat1 descriptors filtered to token's `granted_tools` |
| 4 | `src/executor/tool_registration.rs` | Remove dead `_ =>` fallback from `cat2_tool_descriptor` (or panic — unknown Cat2 names are a bug) |
| 5 | `dispatch_manager.rs:dispatch_via_router` | Wire actual IPC call to `router.svc` socket for Cat1 dispatch |

**Expected outcome**: When `fs.svc` and `llm.svc` are running and have called `ipc.tool-add`,
the LLM receives complete tool descriptors for all granted tools and can call them. No special
`proc/tools/list` discovery tool is needed — the standard tool interface is complete.

**Note**: Fix 5 (router wiring) depends on `router.svc` being implemented. Fixes 1–4 are
independent and unblock correct tool visibility immediately.