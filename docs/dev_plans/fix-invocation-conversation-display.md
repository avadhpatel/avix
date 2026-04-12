# Fix: Agent Invocation Output Not Showing in UI

**Status**: Investigation complete — ready for review  
**Created**: 2026-04-12  
**Priority**: P0 blocker (nothing displays), P1 data quality, P2 robustness

---

## Root Cause Summary

The UI sends `proc/invocation-conversation` to fetch a session's conversation history.
The ATP gateway handler (`gateway/handlers/proc.rs`) logs **"unknown proc op"** for it —
meaning the running binary was compiled from an older revision of the file, before
`"invocation-conversation"` was added to the match arm.

All the *code* is present and correct in source. A rebuild unblocks the immediate issue.
Three secondary issues then determine whether the UI shows useful data.

---

## Issue 1 — P0: Stale Binary (Immediate Blocker)

### Evidence
```
level=WARN  message="unknown proc op"  op="invocation-conversation"
            filename=".../gateway/handlers/proc.rs"  line_number=84
```
The warn at **line 84** is the fallthrough arm. In current source that arm is pushed well
past line 84 by the added ops (`invocation-conversation`, `invocation-snapshot`,
`message-*`, `part-*`). The running binary pre-dates those additions.

### What is in source (already correct, needs no code change)
- `crates/avix-core/src/gateway/handlers/proc.rs` — `"invocation-conversation"` is listed
  in the catch-all IPC-forward arm (alongside `invocation-get`, `invocation-snapshot`, etc.)
- `crates/avix-core/src/kernel/ipc_server.rs` — `"kernel/proc/invocation-conversation"` 
  handler calls `proc_handler.read_invocation_conversation(inv_id)`
- `crates/avix-core/src/kernel/proc/mod.rs` — `read_invocation_conversation()` implemented:
  looks up the invocation record, then delegates to `InvocationStore::read_conversation()`
- `crates/avix-core/src/invocation/store.rs` — `read_conversation()` reads
  `<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl` and parses each line
  as a `ConversationEntry`

### Fix
Rebuild the binary. No source changes required for this issue.

```bash
cargo build --package avix-core --package avix-cli --package avix-app-web
```

---

## Issue 2 — P1: Conversation Written in Flat Format (Tool Calls Lost)

### Problem
`RuntimeExecutor::shutdown_with_status()` (and the interim `save_invocation_state()` path)
both call `InvocationStore::write_conversation()`, which takes `&[(String, String)]` — the
raw `(role, content)` tuples from `MemoryManager.conversation_history`.

```rust
// executor/runtime_executor.rs ~line 611
store.write_conversation(
    &self.invocation_id,
    &self.spawned_by,
    &self.agent_name,
    &self.memory.conversation_history,   // Vec<(String, String)>
).await;
```

This writes:
```jsonl
{"role":"user","content":"Write a Fibonacci function"}
{"role":"assistant","content":"Here is the code..."}
```

Tool calls, thought traces, and file diffs are **never persisted** because
`MemoryManager.conversation_history` is typed as `Vec<(String, String)>` — it has no
capacity to store structured data.

`write_conversation_structured()` exists (takes `&[ConversationEntry]`) but is not called
from the executor.

### Impact
After the rebuild, conversations **will** appear in the UI — role and content will be
correct. But the UI drawer shows only `{ role, content }` anyway (see `HistoryPage.tsx`
line 49), so this is invisible for now. However, this will matter as soon as the UI
starts rendering tool calls.

### Proposed Fix — Two files

**File A: `crates/avix-core/src/executor/memory.rs`**

Change `conversation_history` from `Vec<(String, String)>` to `Vec<ConversationEntry>`:

```rust
use crate::invocation::conversation::{ConversationEntry, Role};

pub struct MemoryManager {
    pub conversation_history: Vec<ConversationEntry>,
    // ...
}
```

Update `add_turn()` to accept a full `ConversationEntry` (or keep a convenience wrapper
for simple `role/content` turns — the LLM dispatch path only needs role+content):

```rust
pub fn add_turn(&mut self, role: impl Into<String>, content: impl Into<String>) {
    self.conversation_history.push(
        ConversationEntry::from_role_content(
            role.into().parse().unwrap_or(Role::User),
            content,
        )
    );
}

pub fn add_structured_turn(&mut self, entry: ConversationEntry) {
    self.conversation_history.push(entry);
}
```

The LLM dispatch path (`dispatch_manager.rs`) currently passes
`message_history: &self.memory.conversation_history` as `&[(String, String)]`. After the
type change, update that slice construction to extract `(role_str, content)` pairs for the
LLM call:

```rust
// dispatch_manager.rs  ~line 132
let history: Vec<(String, String)> = self.memory.conversation_history
    .iter()
    .map(|e| (format!("{:?}", e.role).to_lowercase(), e.content.clone()))
    .collect();
// pass &history instead of &self.memory.conversation_history
```

**File B: `crates/avix-core/src/executor/runtime_executor.rs`**

In `shutdown_with_status()` switch from `write_conversation` to `write_conversation_structured`:

```rust
store.write_conversation_structured(
    &self.invocation_id,
    &self.spawned_by,
    &self.agent_name,
    &self.memory.conversation_history,
).await;
```

Same change in `dispatch_manager.rs::save_invocation_state()` → `persist_interim_structured`.

> **Note**: `InvocationStore::persist_interim()` also calls `write_conversation()`.
> A parallel `persist_interim_structured()` method will be needed (or refactor to accept
> `&[ConversationEntry]`).

### Targeted tests

```bash
cargo test --package avix-core invocation::store
cargo test --package avix-core executor::memory
cargo test --package avix-core executor::runtime_executor
```

---

## Issue 3 — P2: Serial N+1 Round-Trips in `get_session_messages`

### Problem
`routes.rs::get_session_messages` (avix-app-web) fetches conversations with N sequential
ATP calls — one `invocation-conversation` per invocation in the session:

```rust
for inv in &invocations {
    let entries = core_get_invocation_conversation(&dispatcher, inv_id).await…;
}
```

For a session with 5 invocations this is 1 (`invocation-list`) + 5 (`invocation-conversation`)
= 6 round-trips, all sequential. Also explains the duplicate requests in the log —
the web UI is calling `get_session_messages` twice concurrently (two threads).

### Proposed Fix — `routes.rs` only

Parallelize the conversation fetches with `futures::future::join_all`:

```rust
use futures::future::join_all;

let futs: Vec<_> = invocations.iter().map(|inv| {
    let dispatcher = dispatcher.clone();
    let inv_id = inv["id"].as_str().unwrap_or("").to_string();
    async move {
        let entries = core_get_invocation_conversation(&dispatcher, &inv_id)
            .await.unwrap_or_default();
        (inv, entries)
    }
}).collect();

let results = join_all(futs).await;
```

This collapses N sequential waits into one parallel batch. The duplicate-request issue
(two concurrent `get_session_messages` calls for the same session from the web UI) is a
frontend bug and should be fixed there (deduplicate / debounce the fetch).

---

## Issue 4 — P2: Hard Parse Failure on Malformed JSONL Lines

### Problem
`InvocationStore::read_conversation()` propagates parse errors immediately:

```rust
let entry: ConversationEntry = serde_json::from_str(line)
    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;  // <-- hard abort
```

A single malformed line (e.g. a partial write during a crash) makes the entire
conversation unreadable — the `read_invocation_conversation` path returns `Err`, which
causes the IPC handler to return a `-32000` error, and the UI gets nothing.

### Proposed Fix — `invocation/store.rs` only

Skip unreadable lines with a warning:

```rust
for line in text.lines() {
    if line.trim().is_empty() { continue; }
    match serde_json::from_str::<ConversationEntry>(line) {
        Ok(entry) => entries.push(entry),
        Err(e) => {
            warn!(error = %e, line = line, "skipping malformed conversation line");
        }
    }
}
```

---

## Recommended Implementation Order

| Step | File | Priority | Risk |
|------|------|----------|------|
| 0 | **Rebuild the binary** — no source change | P0 | None |
| 1 | `invocation/store.rs` — soft-skip malformed JSONL lines | P2 | Low — isolated change |
| 2 | `executor/memory.rs` — change history type to `Vec<ConversationEntry>` | P1 | Medium — type propagation |
| 3 | `executor/dispatch_manager.rs` — fix history extraction for LLM calls | P1 | Medium — paired with step 2 |
| 4 | `executor/runtime_executor.rs` — switch to `write_conversation_structured` | P1 | Low once steps 2-3 done |
| 5 | `invocation/store.rs` — add `persist_interim_structured` | P1 | Low |
| 6 | `src-web/src/routes.rs` — parallelize conversation fetches | P2 | Low |

Steps 2–5 must be done together (one compile unit of work). Steps 1 and 6 are independent
and can be done before or after.

---

## Architecture Notes (no changes needed)

- The `invocation-conversation` ATP op → IPC flow is correctly designed and complete.
  The "unknown op" error is purely a stale binary artifact.
- The `ConversationEntry` struct (v1 flat + v2 structured) is correctly defined and
  backward-compatible — old flat-format files already parse correctly because all
  structured fields have `#[serde(default)]`.
- ACL: `invocation-conversation` correctly inherits the Proc domain default (`Role::User`).
  No ACL changes required.
