# cap/request-tool — Bug Fixes & Prompt Improvement

## Summary

Three distinct problems were found with the `cap/request-tool` flow:

1. **HIL flow never triggered** — `KernelResourceHandler` always returns `granted: false`
   synchronously for tool requests. SIGPAUSE / `HilManager::open` / ATP event / SIGRESUME
   are never invoked. The entire human-approval path is a stub.
2. **Silent failure when `resource_handler` is `None`** — the tool call returns
   `{ approved: false }` with no error message and no indication to the LLM of why.
3. **Agent re-requests denied tools within the same turn** — after a denial the LLM
   has no instruction preventing it from calling `cap/request-tool` again for the same
   tool, so it loops until the tool chain limit is hit.

## Architecture References

- `docs/architecture/05-capabilities.md` — ResourceRequest / HIL flow, SIGPAUSE/SIGRESUME
- `docs/architecture/09-runtime-executor-tools.md` — Category 2 tools, always-present
  tools, Block 4 (Pending Instructions)

---

## Root Cause Analysis

### Bug 1 — HIL flow is a stub in `KernelResourceHandler`

**Location:** `crates/avix-core/src/kernel/resource_request.rs:234`

`KernelResourceHandler::dispatch_item` for `ResourceItem::Tool` unconditionally returns:

```rust
ResourceGrant::Tool {
    granted: false,
    name: name.clone(),
    new_token: None,
    reason: Some("Requires human-in-the-loop approval".into()),
    suggestion: Some("Send SIGPAUSE and present request to user via cap/request-tool HIL flow".into()),
}
```

The suggestion is documentation, not code. `dispatch_manager.rs` sees `granted: false`
and immediately returns `{ approved: false }` to the LLM. The actual HIL path —
`HilManager::open` (writes VFS file, publishes `hil.request` ATP event, starts timeout
timer), `SignalBus::send(SIGPAUSE)`, and waiting for `SIGRESUME` — is never reached.

**Fix:** `dispatch_manager.rs` must drive the HIL flow directly after calling
`handler.handle()` and receiving `granted: false` for a tool item. The sequence:

1. Call `hil_manager.open(HilRequest { ... })` — writes VFS, fires ATP event, starts
   timeout.
2. Send `SIGPAUSE` to self via `signal_bus`.
3. Await `SIGRESUME` on the `SignalBus` subscription for this PID (same pattern as
   `CapabilityUpgrader::request_tool`).
4. On `decision == "approved"`: re-issue token with new tool, refresh tool list, return
   `{ approved: true }`.
5. On `decision == "denied"` or `"timeout"`: push tool to `denied_tools`, return
   `{ approved: false, reason: "..." }`.

`RuntimeExecutor` must be wired with `HilManager` and `SignalBus` handles (analogous to
`with_resource_handler`). Add `with_hil_manager` and `with_signal_bus` builder methods
if they are not already present.

> **Note:** `CapabilityUpgrader` in `executor/hil/cap_upgrade.rs` already implements
> the SIGRESUME-wait logic correctly. `dispatch_manager.rs` should reuse or call into
> it rather than duplicating the wait loop.

### Bug 2 — No `resource_handler` attached

**Location:** `crates/avix-core/src/executor/runtime_executor/dispatch_manager.rs:381`

When `RuntimeExecutor` is constructed without `.with_resource_handler(handler)` the
`cap/request-tool` arm falls through to `kernel.is_auto_approve()` or returns a silent
`{ approved: false }`.

**Fix:** In the no-handler fallback, return a structured error:

```json
{ "approved": false, "error": "capability escalation unavailable: no resource handler configured" }
```

### Bug 3 — Agent re-requests denied tools within the same turn

**Location:** `crates/avix-core/src/executor/runtime_executor.rs:189` and
`crates/avix-core/src/executor/prompt.rs:34`

`RuntimeExecutor` already has `denied_tools: Vec<String>` but it is never written to on
a denial and never surfaced in the system prompt.

**Fix (two parts):**

**Part A — Record denials in `dispatch_manager.rs`:**

Push the tool name to `self.denied_tools` on every non-approved outcome (denied,
timeout, error, no-handler).

**Part B — Surface in `prompt.rs` Block 4:**

Add `denied_tools: &[String]` to `build_system_prompt`. When non-empty, append:

```
**Tool access denied this turn:** fs/write, send_email
Do not call cap/request-tool for these tools again until the next user message.
```

The denied list is **per-turn** — clear `self.denied_tools` at the start of each new
user-message turn (same place `pending_messages` is cleared today).

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/kernel/resource_request.rs` | Remove stub grant for `ResourceItem::Tool`; document that HIL is driven by the caller |
| 2 | `crates/avix-core/src/executor/runtime_executor/dispatch_manager.rs` | Drive full HIL flow after `granted: false`; push to `denied_tools` on all denial paths; structured error when no handler |
| 3 | `crates/avix-core/src/executor/runtime_executor.rs` | Add `with_hil_manager` / `with_signal_bus` builder methods if missing; clear `denied_tools` on new user-message turn; pass to `build_system_prompt` |
| 4 | `crates/avix-core/src/executor/prompt.rs` | Add `denied_tools: &[String]` param; emit Block 4 warning when non-empty |

---

## Implementation Order

### Step 1 — `resource_request.rs`

Update the `ResourceItem::Tool` arm in `dispatch_item` to remove the misleading
suggestion string and add a clear doc comment that HIL orchestration is the
responsibility of the `dispatch_manager` caller:

```rust
ResourceItem::Tool { name, .. } => ResourceGrant::Tool {
    granted: false,
    name: name.clone(),
    new_token: None,
    reason: Some("HIL approval required".into()),
    suggestion: None,
},
```

Run: `cargo check --package avix-core`
Test: `cargo test -p avix-core kernel::resource_request`

---

### Step 2 — `dispatch_manager.rs`

After receiving `granted: false` for a `ResourceItem::Tool`, orchestrate the HIL flow:

```rust
// pseudo-code — exact API depends on what builder methods exist
if let (Some(hil_mgr), Some(sig_bus)) = (&self.hil_manager, &self.signal_bus) {
    let hil_id = uuid::Uuid::new_v4().to_string();
    hil_mgr.open(HilRequest::capability_upgrade(self.pid, &tool_name, &reason, &hil_id)).await?;
    sig_bus.send(Signal { target: self.pid, kind: SignalKind::Pause, payload: json!({}) }).await.ok();

    // Reuse CapabilityUpgrader wait logic or inline equivalent
    match upgrader.request_tool(&tool_name, &reason, &hil_id, timeout).await {
        Ok(()) => {
            self.token = upgrader.current_token().clone();
            self.refresh_tool_list().await;
            return Ok(json!({"approved": true, "tool": tool_name}));
        }
        Err(e) => {
            self.denied_tools.push(tool_name.clone());
            return Ok(json!({"approved": false, "tool": tool_name, "reason": e.to_string()}));
        }
    }
}
```

Also add the no-handler structured error and push `denied_tools` on all denial paths.

Run: `cargo check --package avix-core`
Test: `cargo test -p avix-core executor::runtime_executor::dispatch_manager::tests::test_dispatch_cap_request_tool`

---

### Step 3 — `runtime_executor.rs`

1. Add `with_hil_manager` and `with_signal_bus` builder methods if not present.
2. Clear `self.denied_tools` at the start of each new user-message turn.
3. Pass `&self.denied_tools` to `build_system_prompt`.

Run: `cargo check --package avix-core`
Test: `cargo test -p avix-core executor::runtime_executor`

---

### Step 4 — `prompt.rs`

Add `denied_tools: &[String]` as last parameter. When non-empty, emit in Block 4:

```rust
if !denied_tools.is_empty() {
    let list = denied_tools.join(", ");
    prompt.push_str(&format!(
        "\n**Tool access denied this turn:** {list}\n\
         Do not call cap/request-tool for these tools again until the next user message.\n"
    ));
}
```

Update all call sites and existing tests.

Run: `cargo check --package avix-core`
Test: `cargo test -p avix-core executor::prompt`

---

## Testing Strategy

- **Step 1:** `cargo test -p avix-core kernel::resource_request` — verify `Tool` arm
  returns `granted: false` with `suggestion: None`.
- **Step 2:** Add `test_dispatch_cap_request_tool_hil_approved` and
  `test_dispatch_cap_request_tool_hil_denied` using a mock `HilManager` and `SignalBus`.
  Verify `denied_tools` is populated on denial. Verify structured error on no-handler.
- **Step 3:** `test_denied_tools_cleared_on_new_turn` — push a denied tool, simulate a
  new user message, assert `denied_tools` is empty.
- **Step 4:** `test_prompt_denied_tools_block4` — assert warning text present when
  `denied_tools` is non-empty; assert absent when empty.

Target: 95%+ coverage on all four files.
