# Dev Plan: Streaming Events Pipeline — Gaps & Fixes

## Task Summary

Agent executor messages are not being streamed to the UI after a session is spawned.

**Gaps A, B, D (client-side) — ✅ COMPLETE** (commit `c7a9dbf`, 2026-04-17):
- Gap A: `ConnectionStatus` hardcoded `"core-init"` session_id → fixed
- Gap B: `pid` type mismatch in typed body structs → fixed
- Gap D: `start_event_bridge()` double-start on reconnect → fixed with `AtomicBool` guard

**Gap C (server-side routing) — ⏳ PENDING** — see [`streaming-events-gap-D-session-id-routing.md`](streaming-events-gap-D-session-id-routing.md):
During investigation a deeper server-side bug was found: `IpcExecutorFactory` passes the
**agent logical session UUID** to `event_bus.*` calls that expect the **ATP connection
session ID**. The ownership gate (`conn.session_id == event.owner_session`) always fails,
silently dropping every streaming event. This is the primary reason streaming does not work.

---

## Architecture Spec References

- `docs/architecture/04-atp.md` — ATP protocol, 3-gate event filter, subscribe frame
- `docs/architecture/13-streaming.md` — End-to-end streaming pipeline spec
- `docs/architecture/07-services.md` — Service lifecycle (gateway.svc)

---

## Full Pipeline (Current State)

### Server Side (broken — see gap-D plan for fix)

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
- ⚠️ `IpcExecutorFactory` currently passes the agent logical session UUID (not the ATP connection session ID) to all `event_bus.*` calls — ownership gate always fails. Fix is in the gap-D plan.

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

### Gap A — `ConnectionStatus` stores wrong session_id ✅ RESOLVED (commit `c7a9dbf`)
**File**: `crates/avix-client-core/src/state.rs`
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

### Gap B — `AgentOutputChunkBody.pid` type annotation was wrong ✅ RESOLVED (commit `c7a9dbf`)
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

### Gap C — `EventBody` missing typed variant for `AgentSpawned` ✅ RESOLVED (commit `c7a9dbf`)
**File**: `crates/avix-client-core/src/atp/types.rs`

`AgentSpawnedBody` struct and `EventBody::AgentSpawned` variant were added. The `sessionId`
field will be populated once gap-D adds it to the server-emitted body.

Note: `AgentSpawnedBody.session_id` is currently a placeholder — the server does not yet
emit `sessionId` in the `agent.spawned` body. That is part of the gap-D plan.

---

### Gap D — `start_event_bridge()` had no double-start guard ✅ RESOLVED (commit `c7a9dbf`)
**File**: `crates/avix-client-core/src/state.rs`

`bridge_started: Arc<AtomicBool>` added to `AppState`. `start_event_bridge()` uses
`compare_exchange(false, true)` — a second call is a no-op. The flag resets to `false`
when the bridge task exits (on disconnect) and when a new connection is established in
`do_connect()`, allowing a clean restart on reconnect.

---

## Multi-User Isolation Design (gate is correct; routing is broken — see gap-D)

All agent events are owner-scoped (`event_scope` returns `owner_scoped = true`). The gateway
only delivers an event to a connection where `conn.session_id == event.owner_session`. The
**gate logic is correct**, but `IpcExecutorFactory` currently sets `owner_session` to the
agent logical session UUID instead of the ATP connection session ID — so the gate always
fails and every event is dropped (see `streaming-events-gap-D-session-id-routing.md`).

Once gap-D is fixed, multi-user isolation is automatically enforced by the ownership gate
with no additional changes needed.

---

## Files to Change

| # | File | Change | Status |
|---|------|--------|--------|
| 1 | `crates/avix-client-core/src/atp/dispatcher.rs` | Add `pub fn session_id(&self) -> &str` | ✅ Done (`c7a9dbf`) |
| 2 | `crates/avix-client-core/src/state.rs` | Use `dispatcher.session_id()` in `do_connect()`; add `bridge_started` guard | ✅ Done (`c7a9dbf`) |
| 3 | `crates/avix-client-core/src/atp/types.rs` | Fix `pid: u64` → `pid: String` in typed body structs; add `AgentSpawnedBody` | ✅ Done (`c7a9dbf`) |

---

## Implementation Order

### Step 1 — ✅ DONE (`c7a9dbf`) — `dispatcher.rs`: Expose `session_id()`

Added public accessor:
```rust
pub fn session_id(&self) -> &str {
    &self.inner.session_id
}
```

---

### Step 2 — ✅ DONE (`c7a9dbf`) — `state.rs`: Fix session_id + bridge guard

Fixed `do_connect()` to use real session_id from dispatcher; added `bridge_started: Arc<AtomicBool>`
with `compare_exchange` idempotency guard in `start_event_bridge()`; reset flag to `false` on
disconnect and on new connection.

---

### Step 3 — ✅ DONE (`c7a9dbf`) — `types.rs`: Fix pid type annotations + add AgentSpawnedBody

Changed `pid: u64` → `pid: String` in `AgentOutputBody`, `AgentOutputChunkBody`, `AgentStatusBody`,
`AgentExitBody`. Added `AgentSpawnedBody { pid, name, goal }` and `EventBody::AgentSpawned` variant.
Updated `avix-cli/src/tui/app.rs` to parse string PIDs at each usage site.

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

- [x] `ConnectionStatus::Connected.session_id` matches the real ATP login session_id (`c7a9dbf`)
- [x] `start_event_bridge()` is idempotent (second call is a no-op) (`c7a9dbf`)
- [x] `agent.output.chunk` body arrives at frontend with `pid` as a string (matching wire format) (`c7a9dbf`)
- [x] Streaming text tokens appear in the SessionPage as the agent executes (`ef603f8` — ATP routing fixed)
- [x] Agent tool calls appear in `liveToolCalls` during execution (emission existed; wrong session_id fixed in `dispatch_manager.rs`)
- [x] Events from one user's session do not appear in another user's UI (`ef603f8`)
