# Dev Plan: Cat1 exec.svc Tool Registration

**Status:** COMPLETE
**Priority:** P1 — Cat1 0d from TODO.md
**Tracks:** `TODO.md` item 0d

---

## Problem Summary

`exec.svc` starts and creates `exec.sock` during bootstrap, but never calls
`tool_registry.add()`. `refresh_tool_list` in `RuntimeExecutor` finds nothing for
`exec/run`, so `cat1_count` stays 0 and the tool never appears in the LLM's tool list.
Agents cannot execute code even though the exec service is running.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` § "Category 1 Direct Service Tools"
- `crates/avix-core/src/exec_svc/ipc_server.rs` — `dispatch()` handles `"exec/run"` method

---

## What exec.svc Exposes

Single JSON-RPC method `exec/run`:
- params: `{ "runtime": "bash"|"python"|"sh", "code": "<source>" }`
- returns: `{ "stdout": str, "stderr": str, "exit_code": i64 }`

Socket: `runtime_dir/exec.sock`

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/bootstrap/mod.rs` | Register `exec/run` in tool registry after exec.svc starts |

---

## Implementation

### Step 1 — `bootstrap/mod.rs`: register exec/run after exec.svc starts

**Location:** immediately after the `exec.svc` start block (the `tracing::info!(sock = %exec_sock.display(), "exec.svc started")` line).

```rust
// Register exec.svc Cat1 tool in the tool registry so agents can discover
// and call it. IPC binding: endpoint "exec" resolves to runtime_dir/exec.sock.
if let Ok(tool_name) = ToolName::parse("exec/run") {
    let descriptor = serde_json::json!({
        "name": "exec/run",
        "description": "Execute code in a sandboxed runtime (bash, python, or sh)",
        "ipc": {
            "transport": "local-ipc",
            "endpoint": "exec",
            "method": "exec/run",
        }
    });
    let entry = ToolEntry::new(
        tool_name,
        "exec.svc".to_string(),
        ToolState::Available,
        ToolVisibility::All,
        descriptor,
    );
    if let Err(e) = tool_registry.add("exec.svc", vec![entry]).await {
        tracing::warn!(error = %e, "failed to register exec.svc tools");
    } else {
        tracing::info!("registered exec.svc tools in tool registry");
    }
}
```

Imports already present from llm.svc registration: `ToolEntry`, `ToolState`,
`ToolVisibility`, `ToolName`.

**Compile check:** `cargo check --package avix-core`

---

## Tests

Add a unit test in `bootstrap/mod.rs` (in the existing `#[cfg(test)]` block) that:
1. Creates a `Runtime` with a temp dir (follow the existing test pattern for `bootstrap_with_root`)
2. Calls `phase3_services()`
3. Asserts `tool_registry.lookup("exec/run")` returns `Ok(entry)` with a non-null `ipc` field

Since `phase3_services` isn't easily testable in isolation (it needs a full `Runtime`),
verify by adding to any existing bootstrap integration test that already exercises phase3.
If no such test exists, add a targeted `#[tokio::test]` to the bootstrap `tests` module.

**Test filter:** `cargo test --package avix-core bootstrap`

---

## Expected Outcome

1. `tool_registry.lookup("exec/run")` → `Ok(entry)` with IPC binding to `exec.sock`
2. LLM tool list includes `exec/run` with `cat1_count >= 1` on first turn
3. Agent can call `exec/run` with `{"runtime":"bash","code":"echo hello"}` and receive stdout
