# Dev Plan: Per-Agent Tool State (Gap Analysis & Fix)

## Task Summary

Agents should see `state: unavailable` in `/tools/**/*.yaml` for tools they lack capability
grants for. The entire infrastructure is already built — permissions, VFS YAML generation,
`request_access` hint, `VfsCallerContext` — but `VfsRouter.set_caller()` is never called,
so the per-agent logic is dead code. This plan wires the single missing call.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` — tool exposure model
- `docs/architecture/04-atp.md` — capability tokens

---

## What Is Already Implemented (No Changes Needed)

| Component | Status | Notes |
|-----------|--------|-------|
| `ToolPermissions` struct (`tool_registry/permissions.rs`) | ✅ Done | rwx fields, default, admin |
| `ToolDescriptor.permissions` field (`descriptor.rs`) | ✅ Done | `Option<ToolPermissions>`, serde |
| Scanner derives permissions from descriptor (`scanner.rs`) | ✅ Done | explicit block > owner field > default |
| `ToolEntry.permissions` + `with_permissions()` (`entry.rs`) | ✅ Done | |
| `VfsRouter.generate_tool_yaml()` (`memfs/router.rs`) | ✅ Done | Computes `state: available/unavailable` from `VfsCallerContext.token`; outputs `permissions:` block; outputs `request_access: cap/request-tool` for unavailable tools |
| `VfsCallerContext` with `token: Option<CapabilityToken>` (`memfs/context.rs`) | ✅ Done | |
| `VfsRouter.set_caller()` method | ✅ Done | Exists but is never called |
| `vfs/tools_provider.rs` — static `state:` in catalog listing | ✅ Done | Uses `entry.state`, not per-agent |

---

## Root Cause

```
VfsRouter.caller: RwLock<Option<VfsCallerContext>>
                          ^^^^
                          Always None — set_caller() is never called from RuntimeExecutor.
```

`generate_tool_yaml` has the logic:
```rust
let state = if let Some(c) = caller {
    // ← this branch NEVER runs
    if let Some(token) = &c.token {
        if caps.iter().all(|cap| token.has_tool(cap)) { "available" } else { "unavailable" }
    } else if c.is_admin { "available" } else { "unavailable" }
} else {
    // ← always falls through to here
    match entry.state { ToolState::Available => "available", ... }
};
```

Every tool entry has `ToolState::Available` by default (set at service install), so every
agent sees every tool as available regardless of their capability token.

---

## What Needs To Change

**One function change** and **two test additions**.

### Gap 1 — `RuntimeExecutor` never sets VFS caller context

`RuntimeExecutor` holds the agent's `CapabilityToken` and `spawned_by` username, and it
holds the `VfsRouter`. It needs to call `vfs.set_caller(...)` at spawn time so subsequent
`/tools/**` reads are filtered against the agent's actual capability grants.

The best place is `spawn_with_registry_ref` right after `refresh_tool_list()` — the VFS
is wired in later via `with_vfs()`, so `set_caller` must be deferred to `with_vfs()`.

### Gap 2 — `with_vfs()` should set caller immediately

`RuntimeExecutor::with_vfs(vfs)` is called from `executor_factory.rs` (production) and
tests. When the VFS is attached, `set_caller` should fire using the already-known token
and spawned_by. This is the right hook point because the token is already available on
`self` by the time `with_vfs` is called.

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/executor/runtime_executor.rs` | In `with_vfs()`, call `vfs.set_caller(Some(VfsCallerContext { ... }))` using `self.token` + `self.spawned_by` |

That is the only production code change required.

---

## Implementation Detail

### `runtime_executor.rs` — `with_vfs()`

Current:
```rust
pub fn with_vfs(mut self, vfs: Arc<VfsRouter>) -> Self {
    self.vfs = Some(vfs);
    self
}
```

Problem: `set_caller` is `async`, but `with_vfs` is sync. Two options:
1. Make `with_vfs` async (breaking change for tests — small, contained)
2. Add a separate `pub async fn init_vfs_caller(&self)` that callers invoke after `with_vfs`

**Option 2 is cleaner** — avoids making all call sites async, and the caller (executor_factory.rs)
already has an async context. Add `init_vfs_caller` and call it in `executor_factory.rs`
right after the `with_vfs` call.

```rust
/// Set the VFS caller context from this executor's token so tool state is
/// computed per-agent (available vs unavailable based on capability grants).
pub async fn init_vfs_caller(&self) {
    let Some(vfs) = &self.vfs else { return };
    let ctx = VfsCallerContext {
        username: self.spawned_by.clone(),
        crews: vec![],          // no crew info available at executor level
        is_admin: false,        // conservative default; capability grants do the real work
        token: Some(self.token.clone()),
    };
    vfs.set_caller(Some(ctx)).await;
    tracing::debug!(
        pid = self.pid.as_u64(),
        spawned_by = %self.spawned_by,
        "VFS caller context set from agent token"
    );
}
```

Call site in `executor_factory.rs` (already in the async task):
```rust
executor = executor.with_vfs(Arc::clone(&vfs));   // already exists
executor.init_vfs_caller().await;                  // NEW — one line
```

**Note on `crews: vec![]`**: The VFS caller context `crews` field is used by `VfsPermissions`
for file read/write access, not for tool state. Tool state is determined solely by
`token.has_tool(cap)`. Leaving crews empty is correct — agents don't need crew membership
for tool access, they need capability grants in their token.

**Note on `is_admin: false`**: Admins already have full capability tokens minted at spawn,
so `token.has_tool(cap)` returns true for all tools. The `is_admin` shortcut in
`generate_tool_yaml` is redundant but harmless.

---

## Also Fix: `executor_factory.rs` doesn't call `with_vfs` at all

Looking at the production path in `executor_factory.rs`:
```rust
executor = executor.with_event_bus(Arc::clone(&event_bus));
executor = executor.with_tracer(Arc::clone(&tracer));
executor = executor.with_invocation_store(invocation_store, invocation_id);
executor = executor.with_session_store(session_store);
```

`with_vfs` is **not called** in `IpcExecutorFactory::launch`. The VFS is only attached in
tests (via `make_executor_with_vfs`). So even after adding `init_vfs_caller`, the
production path won't benefit unless the VFS is wired in.

This requires checking where the `VfsRouter` instance lives in the kernel bootstrap and
whether it should be passed into `IpcExecutorFactory`. Let me verify:

**Architecture invariant**: each agent gets its own VFS view (`/proc/<pid>/`, etc.) but
shares the same underlying providers. The `VfsRouter` in the kernel is a single shared
instance. Per-agent state is achieved by calling `set_caller` with the agent's token
before each read — the `caller` field is a `RwLock`, so it is safe to update per-call.

**Conclusion**: The shared `VfsRouter` can be passed into `IpcExecutorFactory` and
attached to each executor. Before reading `/tools/`, the executor calls `set_caller`
with its own token. This is the correct design.

---

## Updated Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/executor/runtime_executor.rs` | Add `pub async fn init_vfs_caller(&self)` |
| 2 | `crates/avix-core/src/bootstrap/executor_factory.rs` | Add `vfs: Arc<VfsRouter>` field; call `executor.with_vfs(...).init_vfs_caller().await` |
| 3 | `crates/avix-core/src/bootstrap/bootstrap.rs` (or kernel bootstrap) | Pass shared `VfsRouter` into `IpcExecutorFactory::new(...)` |

---

## Implementation Order

### Step 1 — `runtime_executor.rs`: Add `init_vfs_caller()`

Add the method. No call sites yet (tests verify the method works in isolation).

Compile: `cargo check --package avix-core`  
Test: `cargo test --package avix-core -- runtime_executor::proc_manager`

---

### Step 2 — `executor_factory.rs`: Wire VFS + call `init_vfs_caller`

1. Add `vfs: Arc<VfsRouter>` field to `IpcExecutorFactory`
2. Update `IpcExecutorFactory::new(...)` to accept and store it
3. In `launch()`, attach vfs and call `init_vfs_caller`:
   ```rust
   executor = executor.with_vfs(Arc::clone(&self.vfs));
   executor.init_vfs_caller().await;
   ```

Compile: `cargo check --package avix-core`  
Test: `cargo test --package avix-core -- bootstrap::executor_factory`

---

### Step 3 — Bootstrap: Pass `VfsRouter` into `IpcExecutorFactory`

Find the kernel bootstrap where `IpcExecutorFactory::new(...)` is constructed.
Pass the shared `VfsRouter` instance.

Compile: `cargo check --package avix-core`  
Test: `cargo test --package avix-core -- bootstrap::`

---

## Testing Strategy

**Step 1 test** — add to `proc_manager.rs` tests:
```rust
#[tokio::test]
async fn init_vfs_caller_sets_token_on_vfs() {
    let (executor, vfs) = make_executor_with_vfs(910).await;
    executor.init_vfs_caller().await;
    let ctx = vfs.caller().await;
    assert!(ctx.is_some());
    assert!(ctx.unwrap().token.is_some());
}
```

**Step 2 test** — verify that when an executor has a VFS attached and a token with a
specific cap grant, reading a tool YAML for that tool returns `state: available`, and for
a tool requiring a cap the token doesn't have, returns `state: unavailable`.

Add to `bootstrap::executor_factory` tests:
```rust
// After attaching VFS + calling init_vfs_caller, mount a mock tool entry
// requiring cap "agent:kill". Executor token doesn't grant "agent:kill".
// Reading /tools/kernel/proc/kill.yaml should show "state: unavailable".
```

---

## Success Criteria

✅ COMPLETE (commit `accb915`, 2026-04-17)

- [x] `VfsRouter.caller` is set to the agent's `VfsCallerContext` when `init_vfs_caller` is called
- [x] Agent reading `/tools/fs/read.yaml` sees `state: available` if token grants `fs/read`
- [x] Agent reading `/tools/kernel/proc/kill.yaml` sees `state: unavailable` if token lacks `agent:kill`
- [x] Unavailable tool YAML includes `request_access: cap/request-tool`
- [x] No change to tools that have no `capabilities_required` (still `available`)
- [x] `cargo check --package avix-core` clean, all new tests pass

---

## What This Does NOT Change

- `ToolPermissions` / permissions rwx model — already fully implemented, no changes
- `VfsCallerContext::from_token()` (reads users.yaml for crews) — not used here; we build
  the context inline from token + spawned_by
- `sys/tools` Cat2 dispatch — already passes `&self.token` to `kernel.list_tools()` with
  `granted_only` flag; that path is independent and not broken
- The static catalog listing (`vfs/tools_provider.rs`) — not per-agent, not changed
