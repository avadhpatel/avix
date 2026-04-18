# Dev Plan: Cat1 Tool Pipeline Fix

**Status:** COMPLETE (commit `4a126ba`)
**Priority:** P0 — crashes production agents

---

## Problem Summary

Running any agent crashes after 43–50 tool calls with:
```
SYSTEM [Agent stopped: config parse error: exceeded max tool chain limit of 50]
```

Three bugs combine to produce the loop:

### Bug 1 — `validation.rs:7`: `ALWAYS_PRESENT` is missing `sys/tools` (crash root cause)

```rust
// CURRENT — 4 tools, sys/tools absent
const ALWAYS_PRESENT: &[&str] = &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"];
```

`CapabilityToolMap::always_present()` correctly lists all 5 always-present tools including
`sys/tools`. `compute_cat2_tools()` correctly registers it at spawn. But `validate_tool_call`
uses its own local constant that is out of sync. When the LLM calls `sys/tools` it fails with
`capability denied: Tool not granted: sys/tools` even though the tool is registered and has a
handler.

**Cascade:** agent knows from `cap/list` that it has `["fs/read", "fs/write", "agent/spawn",
"llm/complete"]` but its tool list only shows 6 Cat2 tools. Unable to discover tools via
`sys/tools`, the agent loops: denied → `cap/request-tool` → denied → `cap/escalate` → denied
→ repeat 5 calls/turn × ~10 turns → 50-call wall → crash.

### Bug 2 — `llm/complete` not registered in tool registry

`llm/complete` is granted in the hardcoded token. `refresh_tool_list` calls
`reg.lookup("llm/complete")` every turn and finds nothing → `cat1_count = 0` → the tool never
appears in the LLM's tool list. If the LLM manages to call it anyway, `dispatch_cat1_via_registry`
returns `"tool not found in registry"`.

`llm.svc` IS running (logs show xai API calls working). It handles `llm/complete` on its socket.
It just never calls `ipc.tool-add` so the registry is never populated.

### Bug 3 — `proc/agent.rs:142-152`: hardcoded token grants `fs/read`/`fs/write` that can't dispatch

```rust
let token = CapabilityToken::mint(
    vec!["fs/read", "fs/write", "agent/spawn", "llm/complete"],
    ...
);
```

`fs/read` and `fs/write` are in the token but:
- Not in the tool registry (so Cat1 lookup fails)
- No fs.svc socket exists (no dispatch path even if registered)
- Kernel IPC server does not handle `fs/read` (only `kernel/fs/read` is in the SyscallRegistry
  but even those are not handled by the kernel IPC server)

The LLM sees them in `cap/list` output, tries to call them, and gets errors. This confuses
the agent and wastes tool chain budget.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` § "Category 1 Direct Service Tools",
  § "Cat1 Descriptor Discovery", § "Category 2 Avix Behaviour Tools" (Invariant 13),
  § "Capability-to-Tool Mapping"

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/executor/validation.rs` | Add `"sys/tools"` to `ALWAYS_PRESENT` |
| 2 | `crates/avix-core/src/bootstrap/mod.rs` | Register `llm/*` tools in tool registry after `llm.svc` starts |
| 3 | `crates/avix-core/src/kernel/proc/agent.rs` | Remove `fs/read`/`fs/write` from hardcoded token; keep `agent/spawn` + `llm/complete` |

No architecture spec changes needed — the spec is correct; this is a conformance fix.

---

## Implementation Order

### Step 1 — `validation.rs`: add `sys/tools` to `ALWAYS_PRESENT`

**File:** `crates/avix-core/src/executor/validation.rs`

Change the constant at the top of the file:

```rust
// BEFORE
const ALWAYS_PRESENT: &[&str] = &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"];

// AFTER
const ALWAYS_PRESENT: &[&str] = &[
    "cap/request-tool",
    "cap/escalate",
    "cap/list",
    "job/watch",
    "sys/tools",
];
```

This is the only change needed to stop the crash loop.

**Tests to add/update:**
- The existing test `always_present_tools_bypass_capability_check_when_fresh` iterates over
  `ALWAYS_PRESENT` — extend it to verify `sys/tools` is in the list.
- Add a new focused test: token without `sys/tools` in `granted_tools` → `validate_tool_call`
  returns `Ok(())` for a `sys/tools` call.

**Compile check:** `cargo check --package avix-core`
**Test filter:** `cargo test --package avix-core executor::validation`

---

### Step 2 — `bootstrap/mod.rs`: register `llm/*` tools in tool registry

**File:** `crates/avix-core/src/bootstrap/mod.rs`

In `phase3_services`, immediately after the `llm.svc` start block (where `_handle` is stored),
register the llm tool descriptors in the tool registry.

The IPC binding format (from `dispatch_cat1_tool` in `executor/ipc_dispatch.rs`):
- `endpoint` = socket name without `.sock` → resolves to `runtime_dir/<endpoint>.sock`
- `method` = JSON-RPC method name to call on the service

Tools to register (the ones `llm.svc` handles per `llm_svc/service.rs`):

```rust
// After llm.svc starts successfully, register its tools in the tool registry.
// IPC binding: endpoint "llm" → runtime_dir/llm.sock; method = JSON-RPC method name.
let llm_tools = vec![
    ("llm/complete",         "Generate a completion from the language model"),
    ("llm/embed",            "Generate embedding vectors for text input"),
    ("llm/generate-image",   "Generate an image from a text prompt"),
    ("llm/generate-speech",  "Convert text to speech audio"),
    ("llm/transcribe",       "Transcribe audio to text"),
];
let llm_sock_endpoint = "llm";
let mut llm_entries = Vec::new();
for (name, desc) in &llm_tools {
    if let Ok(tool_name) = ToolName::parse(name) {
        let descriptor = serde_json::json!({
            "name": name,
            "description": desc,
            "ipc": {
                "transport": "local-ipc",
                "endpoint": llm_sock_endpoint,
                "method": name,
            }
        });
        llm_entries.push(
            ToolEntry::new(
                tool_name,
                "llm.svc".to_string(),
                ToolState::Available,
                ToolVisibility::All,
                descriptor,
            )
        );
    }
}
if !llm_entries.is_empty() {
    if let Err(e) = tool_registry.add("llm.svc", llm_entries).await {
        tracing::warn!(error = %e, "failed to register llm.svc tools in registry");
    } else {
        tracing::info!("registered llm.svc tools in tool registry");
    }
}
```

Place this block inside the `Ok(_handle) => { ... }` arm so tools are only registered when
`llm.svc` actually started successfully.

Imports to add (if not already present):
```rust
use crate::tool_registry::{ToolEntry, ToolState, ToolVisibility};
use crate::types::tool::ToolName;
```

**Tests to add:**
- Unit test in bootstrap or integration: after `bootstrap_with_root` + `start_daemon` on a
  temp root with `etc/llm.yaml`, `tool_registry.lookup("llm/complete")` returns `Ok(entry)`
  with a non-null `ipc` binding. Because `start_daemon` is hard to unit-test in isolation,
  a targeted integration test in `crates/avix-tests-integration` or a smaller bootstrap-level
  test is acceptable.
- Minimal acceptable: after `phase3_services` runs (extract that call for testability if needed),
  verify the registry contains `llm/complete`.

**Compile check:** `cargo check --package avix-core`
**Test filter:** `cargo test --package avix-core bootstrap`

---

### Step 3 — `proc/agent.rs`: clean up hardcoded token

**File:** `crates/avix-core/src/kernel/proc/agent.rs`

Change the hardcoded `CapabilityToken::mint` call (currently lines 142-152):

```rust
// BEFORE — grants fs/read, fs/write which have no dispatch path
let token = CapabilityToken::mint(
    vec![
        "fs/read".to_string(),
        "fs/write".to_string(),
        "agent/spawn".to_string(),
        "llm/complete".to_string(),
    ],
    Some(issued_to),
    3600,
    &self.master_key,
);

// AFTER — only grant tools that actually work
let token = CapabilityToken::mint(
    vec![
        "agent/spawn".to_string(),
        "llm/complete".to_string(),
        "llm/embed".to_string(),
    ],
    Some(issued_to),
    3600,
    &self.master_key,
);
```

`agent/spawn` is Cat2 (works). `llm/complete` and `llm/embed` are Cat1 via llm.svc (works
after Step 2). `fs/read` and `fs/write` are removed because there is no `fs.svc` socket and
the kernel IPC server does not handle them. The agent won't attempt to call tools it can't use,
and `cap/list` will reflect an accurate picture.

**Note:** This is a temporary placeholder. The correct long-term fix is to resolve the token
from the agent manifest's `requestedCapabilities`. That is deferred to a follow-up plan.

**Tests to update:**
- `spawn_with_factory_launches_executor_task` and any tests that assert the token contains
  `"fs/read"` or `"fs/write"` must be updated to match the new 3-tool set.

**Compile check:** `cargo check --package avix-core`
**Test filter:** `cargo test --package avix-core kernel::proc`

---

## Expected Outcome After All 3 Steps

1. Agent calls `sys/tools` → returns the registered tool list (kernel syscalls + llm.svc tools)
2. Agent calls `cap/list` → shows `["agent/spawn", "llm/complete", "llm/embed"]`
3. Agent calls `llm/complete` with a prompt → dispatched to `llm.sock` → LLM response returned
4. No capability denied loops; agent completes normally
5. `toolCallsTotal` stays well under 50

---

## Out of Scope (follow-up plans)

- **Token resolution from manifest `requestedCapabilities`** — the hardcoded token in
  `proc/agent.rs` should be replaced with proper resolution of the manifest's
  `requestedCapabilities` into tool names. Deferred.
- **`fs.svc` / VFS Cat1 tools** — `fs/read`, `fs/write`, `fs/list` etc. require a VFS
  service that registers via `ipc.tool-add`. No such service exists yet. Deferred.
- **`kernel/fs/*` kernel IPC handlers** — the SyscallRegistry defines `kernel/fs/read`
  etc. but the kernel IPC server doesn't handle them. Deferred.
- **`exec.svc` Cat1 tools** — same pattern as llm.svc but for exec. Deferred.
