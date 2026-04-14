# Dev Plan: Streaming Events Pipeline — Gaps & Fixes

## Task Summary

Agent executor messages are not being streamed to the UI after a session is spawned. The
server-side pipeline (Provider SSE → RuntimeExecutor → AtpEventBus → gateway.svc) is
architecturally complete. The client-side reception pipeline (Dispatcher → start_event_bridge
→ emit_callback → frontend) is also structurally sound — the subscribe frame is sent, the
bridge task is started, and the `Raw(Value)` fallback in `EventBody` means events flow through
even without typed deserialization matches.

The confirmed gaps are narrower: a hardcoded session_id in `ConnectionStatus`, a fragile
bridge-start guard, and a type annotation inconsistency in `AgentOutputChunkBody`.

This plan documents the full pipeline, all identified gaps, and the exact changes needed.

---

## Architecture Spec References

- `docs/architecture/04-atp.md` — ATP protocol, 3-gate event filter, subscribe frame
- `docs/architecture/13-streaming.md` — End-to-end streaming pipeline spec
- `docs/architecture/07-services.md` — Service lifecycle (gateway.svc)

---

## Full Pipeline (Current State)

### Server Side (correct, no changes needed)

```
Provider HTTP SSE
  └─► DirectHttpLlmClient::stream_complete()          [avix-core/src/llm/]
        └─► IPC llm.stream.chunk notifications
              └─► IpcLlmClient recv loop
                    └─► RuntimeExecutor::run_turn_streaming()
                          │  emits per text token:
                          └─► AtpEventBus::agent_output_chunk(
                                  atp_session_id,   ← spawning conn's session_id
                                  pid, turn_id, text_delta, seq, is_final
                              )
                                └─► broadcast::Sender<AtpEvent>
                                      └─► gateway.svc reader
                                            │  3-gate filter:
                                            │  1. role gate (User+)
                                            │  2. ownership gate: conn.session_id == event.owner_session
                                            │  3. subscription gate: client subscribed to "*"
                                            └─► WebSocket → ATP client
```

**Key facts:**
- `AtpEvent.body` for `agent.output.chunk` is `{"pid": pid.to_string(), "turn_id": ..., "text_delta": ..., "seq": ..., "is_final": ...}`
- All agent events (spawned, output, tool_call, tool_result, exit, status) are **owner-scoped** (`event_scope` returns `owner_scoped = true`)
- The ownership gate requires `conn.session_id == event.owner_session`; the connection's `session_id` is set at WebSocket upgrade from the JWT, so it equals `login_resp.session_id`

### Client Side (has gaps — see below)

```
ATP WebSocket
  └─► AtpClient (avix-client-core)
        │  send subscribe frame: {"frame_type":"subscribe","events":["*"]}   ← DONE in connect()
        └─► Dispatcher::new(client)
              │  stores: inner.session_id = client.session.session_id       ← real ATP session_id
              └─► reader task: frames → broadcast::Sender<Event>
                    └─► start_event_bridge() tokio task
                          │  rx = dispatcher.events()  (resubscribe)
                          └─► match event.kind → event_name string
                                └─► serde_json::to_value(&event.body)
                                      └─► (emit_callback)(event_name, &data)
                                            ├─► Tauri: app_handle.emit(event_name, data)
                                            └─► Web: broadcast::Sender<String> → /api/events WS

Frontend (AppContext.tsx)
  └─► listen("agent.output.chunk", handler)
        └─► accumulate pendingChunks[turn_id], update streamingOutputs[pid]
```

---

## Identified Gaps

### Gap A — `ConnectionStatus` stores wrong session_id  
**File**: `crates/avix-client-core/src/state.rs` lines 146–148  
**File**: `crates/avix-client-core/src/atp/dispatcher.rs`

`do_connect()` hardcodes the session_id in `ConnectionStatus::Connected`:
```rust
// CURRENT (broken):
self.connection_status = ConnectionStatus::Connected {
    session_id: "core-init".to_string(),
};
```

The real ATP session_id lives in `Dispatcher::inner.session_id` (captured from
`client.session.session_id` in `Dispatcher::new()`), but `Dispatcher` exposes no public
accessor for it.

**Impact**: Command handlers check `connection_status.session_id()` only for `Some(...)` and
don't use the value, so commands still work. However, this is incorrect state that will cause
bugs in any future code that reads the actual session_id from `ConnectionStatus` (e.g., for
event routing, re-authentication, or session resumption).

**Fix**:
1. Add `pub fn session_id(&self) -> &str { &self.inner.session_id }` to `impl Dispatcher`
2. In `do_connect()`, replace `"core-init".to_string()` with `dispatcher.session_id().to_string()`

---

### Gap B — `AgentOutputChunkBody.pid` type annotation is wrong (cosmetic only)
**File**: `crates/avix-client-core/src/atp/types.rs`

The server deliberately emits `pid` as a **JSON string** across all event bodies (e.g.,
`"pid": pid.to_string()`). This is intentional: JavaScript's `JSON.parse()` loses precision
on u64 values above `Number.MAX_SAFE_INTEGER` (2⁵³−1), so string-encoding is the correct
wire format for PIDs.

However, `AgentOutputChunkBody` declares `pid: u64`, which expects a JSON integer. Serde
fails to match this variant, falls through to `EventBody::Raw(Value)`, and the full body
JSON is preserved and forwarded correctly. The frontend receives `pid` as a JavaScript string,
which works fine as a map key in `pendingChunks`.

**Impact**: No event delivery breakage — `Raw(Value)` handles it correctly. The only issue is
that the `AgentOutputChunkBody` struct is misleading and would silently deserialize to `Raw`
when accessed programmatically from Rust.

**Fix**: Change `pid: u64` to `pid: String` in `AgentOutputChunkBody` (and any other typed
body structs that include `pid`). No server changes needed.

This is **lower priority** — it's a type annotation fix, not a functional gap.

---

### Gap C — `EventBody` missing typed variant for `AgentSpawned`  
**File**: `crates/avix-client-core/src/atp/types.rs`

`EventBody` enum has no `AgentSpawned(...)` variant. The spawned event falls through to
`EventBody::Raw(Value)`. Not critical (the raw Value is forwarded correctly), but prevents
typed access.

**Impact**: Low — frontend accesses `.pid`, `.agentName`, `.sessionId` via raw JSON, which
works in JavaScript.

**Fix**: Add `AgentSpawnedBody` struct and `EventBody::AgentSpawned(AgentSpawnedBody)` variant.
Fields: `pid: u64`, `agent_name: String`, `session_id: String`.

This is **lower priority** than Gaps A and D — document it but implement last.

---

### Gap D — `start_event_bridge()` has no double-start guard
**File**: `crates/avix-client-core/src/state.rs`

`start_event_bridge()` can be called by both `do_connect()` and `set_emit_callback()`. The
current code-paths prevent a double-start (each checks the other's precondition), but this
invariant is fragile. If `login()` is called after the bridge is already running (e.g., a
reconnect), a second bridge task would be spawned, causing each event to fire the callback
twice.

**Impact**: Medium — currently prevented by control flow, but a reconnect scenario would
break it.

**Fix**: Add `bridge_started: Arc<AtomicBool>` to `AppState`. In `start_event_bridge()`,
use `compare_exchange(false, true)` to make the spawn idempotent. Reset to `false` when
`dispatcher` is cleared (on disconnect/reconnect).

---

## Multi-User Isolation Design (already correct server-side)

All agent events are owner-scoped (`event_scope` returns `owner_scoped = true`). The gateway
only delivers an event to a connection where `conn.session_id == event.owner_session`. The
`RuntimeExecutor` sets `owner_session = spawning_connection.session_id` at agent creation.

No changes needed for multi-user isolation — the ATP ownership gate already enforces it.
Fixing Gap A merely ensures the client's `ConnectionStatus` accurately reflects the real
session_id for logging and debugging purposes.

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-client-core/src/atp/dispatcher.rs` | Add `pub fn session_id(&self) -> &str` |
| 2 | `crates/avix-client-core/src/state.rs` | Use `dispatcher.session_id()` in `do_connect()`; add `bridge_started` guard |
| 3 | `crates/avix-client-core/src/atp/types.rs` | Fix `pid: u64` → `pid: String` in typed body structs; add `AgentSpawnedBody` *(lower priority)* |

---

## Implementation Order

### Step 1 — `dispatcher.rs`: Expose `session_id()`

Add a public method:
```rust
pub fn session_id(&self) -> &str {
    &self.inner.session_id
}
```

Compile check: `cargo check --package avix-client-core`

---

### Step 2 — `state.rs`: Fix session_id + bridge guard

In `do_connect()`, change:
```rust
// Before:
self.connection_status = ConnectionStatus::Connected {
    session_id: "core-init".to_string(),
};

// After:
let session_id = dispatcher.session_id().to_string();
self.dispatcher = Some(dispatcher);
self.connection_status = ConnectionStatus::Connected { session_id };
```

Add `bridge_started: Arc<AtomicBool>` to `AppState`. In `start_event_bridge()`:
```rust
if self.bridge_started.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
    tracing::debug!("event bridge already running, skipping");
    return;
}
let bridge_flag = Arc::clone(&self.bridge_started);
tokio::spawn(async move {
    while let Ok(event) = rx.recv().await {
        // ... existing mapping logic ...
    }
    bridge_flag.store(false, Ordering::SeqCst);  // allow restart after disconnect
});
```

Also reset `bridge_started` to `false` wherever `dispatcher` is set to `None` (on disconnect).

Compile check: `cargo check --package avix-client-core`  
Test: `cargo test --package avix-client-core`

---

### Step 3 (optional) — `types.rs`: Fix pid type annotations + add AgentSpawnedBody

In `crates/avix-client-core/src/atp/types.rs`, fix typed body structs to match the
string-encoded PID wire format:

```rust
// AgentOutputChunkBody — change:
pub pid: u64,    // was wrong
// to:
pub pid: String, // matches "pid": pid.to_string() on wire
```

Apply the same `pid: String` fix to any other typed body structs (AgentOutputBody,
AgentStatusBody, AgentExitBody) that include a pid field.

Also add `AgentSpawnedBody`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpawnedBody {
    pub pid: String,
    pub agent_name: String,
    pub session_id: String,
}
```

Add `EventBody::AgentSpawned(AgentSpawnedBody)` before `Raw(Value)` in the untagged enum.

Compile check: `cargo check --package avix-client-core`

---

## Testing Strategy

After completing Steps 1–3, verify end-to-end:
1. Start the daemon: `AVIX_MASTER_KEY=... ./target/debug/avix start --root ...`
2. Launch the app, confirm it connects
3. Spawn an agent, open the session view
4. Confirm `agent.output.chunk` events arrive in the UI as the agent runs
5. Check browser/Tauri devtools for events firing on `pendingChunks` with numeric `pid`

Targeted test commands:
```bash
cargo test --package avix-client-core -- atp::
cargo test --package avix-core -- gateway::
```

---

## Success Criteria

- [ ] `ConnectionStatus::Connected.session_id` matches the real ATP login session_id
- [ ] `start_event_bridge()` is idempotent (second call is a no-op)
- [ ] `agent.output.chunk` body arrives at frontend with `pid` as a string (matching wire format)
- [ ] Streaming text tokens appear in the SessionPage as the agent executes
- [ ] Agent tool calls appear in `liveToolCalls` during execution
- [ ] Events from one user's session do not appear in another user's UI (ownership gate enforcement already correct server-side)
