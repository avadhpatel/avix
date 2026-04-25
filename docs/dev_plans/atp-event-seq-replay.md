# ATP Event Sequence & Replay Buffer

## Problem

Client disconnects and reconnects → all events during disconnect are lost. No recovery path
for missed events. `AtpEventBus` is a live `broadcast` fanout with no persistence.

## Solution

1. Add `seq` (monotonic u64) to every ATP event frame.
2. Add `since_seq: Option<u64>` to the subscribe frame.
3. Gateway maintains an in-memory ring buffer of recent events.
4. On subscribe with `since_seq`, replay missed events before resuming live delivery.

`agent.output.chunk` is excluded from the ring buffer — high volume; clients wanting
agent replay use `invocation-get` → `conversation.jsonl` instead.

## Architecture Spec References

- `docs/architecture/04-atp.md` — ATP protocol, event frames, subscribe frame
- `docs/architecture/13-streaming.md` — streaming / chunk events

---

## Wire Format Changes

### `event` frame (server → client)
```json
{
  "type":      "event",
  "seq":       1042,
  "event":     "agent.output",
  "sessionId": "sess-abc-123",
  "ts":        "2026-04-24T10:00:00Z",
  "body":      { "pid": 57, "text": "..." }
}
```
`seq` is a gateway-global monotonic counter. Starts at 0. Never resets within a process lifetime.

### `subscribe` frame (client → server)
```json
{
  "type":      "subscribe",
  "events":    ["agent.output", "agent.status"],
  "since_seq": 1038
}
```
`since_seq` is optional. When present: gateway replays all buffered events with
`seq > since_seq` that pass the connection's EventFilter, then resumes live delivery.

---

## Files to Change

### Step 1 — `crates/avix-core/src/gateway/atp/frame.rs`

**Changes:**
- `AtpEvent`: add `pub seq: u64` field
- `AtpEvent::new`: keep as-is; seq is assigned by `AtpEventBus::publish` before sending
- `AtpSubscribe`: add `pub since_seq: Option<u64>` (`#[serde(default)]`)

```rust
// AtpEvent — add field
pub seq: u64,

// AtpSubscribe — add field
#[serde(default)]
pub since_seq: Option<u64>,
```

**Tests to add** (in existing `tests` module):
- `seq_field_serializes_in_event_frame` — serialize AtpEvent with seq=5, verify JSON has `"seq":5`
- `since_seq_deserializes_from_subscribe_frame` — parse subscribe JSON with `since_seq`
- `since_seq_defaults_to_none_when_absent` — parse subscribe JSON without `since_seq`

---

### Step 2 — `crates/avix-core/src/gateway/event_bus.rs`

**Changes:**

Add to `BusEvent`:
```rust
pub seq: u64,
```

Add to `AtpEventBus`:
```rust
seq_counter: Arc<AtomicU64>,
ring: Arc<Mutex<VecDeque<BusEvent>>>,
```

New constant:
```rust
const RING_CAPACITY: usize = 512;
```

`AtpEventBus::new()`: init counter (start at 0) and empty VecDeque.

`AtpEventBus::publish()`:
1. Increment `seq_counter` atomically (`fetch_add(1, Ordering::Relaxed)` — post-increment, first event gets seq=0)
2. Assign seq to event's `AtpEvent.seq` field before send
3. Lock ring, push_back the `BusEvent` (with seq), evict front if len > RING_CAPACITY
4. Release lock, then `tx.send(bus_event)`
5. Skip ring storage for `AtpEventKind::AgentOutputChunk` (high volume)

New method:
```rust
pub fn replay_since(&self, since_seq: u64) -> Vec<BusEvent> {
    let ring = self.ring.blocking_lock();  // called from async via spawn_blocking or use tokio Mutex
    ring.iter()
        .filter(|e| e.seq > since_seq)
        .cloned()
        .collect()
}
```
Use `tokio::sync::Mutex` for `ring` (async context). Method becomes `async`.

**Tests to add:**
- `seq_increments_monotonically` — publish 3 events, check seq 0, 1, 2
- `ring_buffer_stores_events` — publish 3, replay_since(0) returns all 3
- `ring_buffer_evicts_oldest` — publish RING_CAPACITY+2 events, replay_since(0) returns exactly RING_CAPACITY
- `replay_since_filters_by_seq` — publish 5 events, replay_since(3) returns only seq 4 and 5
- `output_chunk_excluded_from_ring` — publish AgentOutputChunk, ring stays empty
- `ring_is_ordered` — replay_since returns events in ascending seq order

---

### Step 3 — `crates/avix-core/src/gateway/server.rs`

**Changes in `handle_text_frame`:**

Current subscribe branch:
```rust
Ok(AtpFrame::Subscribe(sub)) => {
    filter.write().await.set_subscriptions(sub.events);
}
```

New subscribe branch:
```rust
Ok(AtpFrame::Subscribe(sub)) => {
    let since_seq = sub.since_seq;
    filter.write().await.set_subscriptions(sub.events);
    // Replay missed events if client provided a cursor
    if let Some(seq) = since_seq {
        let replayed = state.event_bus.replay_since(seq).await;
        let f = filter.read().await;
        for bus_event in replayed {
            if f.should_receive(&bus_event) {
                if let Ok(s) = serde_json::to_string(&bus_event.event) {
                    let _ = tx.send(WsOutMsg::Text(s)).await;
                }
            }
        }
    }
}
```

`handle_text_frame` already receives `tx` and `state` — no signature change needed.

**Tests to add** (in `crates/avix-core/tests/gateway_transport.rs`):
- `subscribe_with_since_seq_replays_missed_events` — connect, publish 3 events to bus,
  subscribe with `since_seq: 0`, verify client receives all 3 replayed
- `subscribe_without_since_seq_no_replay` — subscribe without `since_seq`, only live events
- `replay_respects_event_filter` — publish event for session-A, connect as session-B,
  subscribe with since_seq, verify session-B does NOT receive session-A's owner-scoped event

---

### Step 4 — `crates/avix-client-core/src/atp/types.rs`

**Change:** Add `seq: u64` to the client-side `Event` struct.

```rust
pub seq: u64,
```

**Tests to add:**
- `event_deserializes_seq_field` — parse event JSON with seq, verify field populated

---

### Step 5 — `crates/avix-client-core/src/atp/event_emitter.rs`

**Goal:** Track `last_seq` across reconnects; pass it as `since_seq` in the subscribe frame
on every reconnect so missed events are replayed automatically.

**`connect_fn` signature change:**

Current: `Fn() -> Fut`
New: `Fn(Option<u64>) -> Fut`

The `Option<u64>` is `since_seq` — `None` on first connect, `Some(last_seq)` on every
subsequent reconnect. Callers are responsible for including `since_seq` in their subscribe
frame (sent inside `connect_fn` after WS upgrade). This keeps the subscribe timing
correct: subscribe + cursor are sent atomically as part of connection setup, before
`EventEmitter` starts forwarding events.

**`EventEmitter` struct changes:**

```rust
pub struct EventEmitter {
    rx: broadcast::Receiver<Event>,
    connected: Arc<AtomicBool>,
    last_seq: Arc<tokio::sync::Mutex<Option<u64>>>,
    _handle: JoinHandle<()>,
}
```

**Inner loop changes:**

```rust
let handle = tokio::spawn(async move {
    let mut backoff = Duration::from_secs(1);
    loop {
        let cursor = *last_seq_c.lock().await;         // None first time
        let disp_res = connect_fn(cursor).await;
        if let Ok(disp) = disp_res {
            connected_c.store(true, Ordering::SeqCst);
            let mut disp_rx = disp.events();
            loop {
                match disp_rx.recv().await {
                    Ok(event) => {
                        // Track highest seq seen
                        let mut guard = last_seq_c.lock().await;
                        *guard = Some(match *guard {
                            Some(prev) => prev.max(event.seq),
                            None => event.seq,
                        });
                        drop(guard);
                        let _ = tx_c.send(event);
                    }
                    // ... existing lag/closed handling
                }
            }
            connected_c.store(false, Ordering::SeqCst);
            backoff = Duration::from_secs(1);
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(Duration::from_secs(60));
    }
});
```

**New public accessor:**
```rust
pub async fn last_seq(&self) -> Option<u64> {
    *self.last_seq.lock().await
}
```

**Callers to update:**
- Only test code in `event_emitter.rs` currently calls `EventEmitter::start` — update both
  test closures to accept `Option<u64>` parameter (ignore it in tests).
- Real callers (CLI session connect, TUI connect) will wire `since_seq` through their
  subscribe frame construction inside `connect_fn`.

**Tests to add:**
- `connect_fn_receives_none_on_first_connect` — verify first call gets `None`
- `connect_fn_receives_last_seq_on_reconnect` — simulate disconnect after receiving
  event with seq=5, verify second `connect_fn` call receives `Some(5)`
- `last_seq_tracks_highest_seq` — receive events seq=3, seq=7, seq=5 (out of order),
  verify `last_seq()` returns `Some(7)`

---

## Implementation Order

1. `frame.rs` (Step 1) — seq on event frame, since_seq on subscribe frame
2. `event_bus.rs` (Step 2) — ring buffer + seq counter
3. `server.rs` (Step 3) — replay on subscribe
4. `avix-client-core/atp/types.rs` (Step 4) — seq on client Event struct
5. `avix-client-core/atp/event_emitter.rs` (Step 5) — last_seq tracking + reconnect cursor

---

## Testing Strategy

| Step | Cargo filter |
|------|-------------|
| 1 | `cargo test -p avix-core --lib gateway::atp::frame` |
| 2 | `cargo test -p avix-core --lib gateway::event_bus` |
| 3 | `cargo test -p avix-core --test gateway_transport` |
| 4 | `cargo test -p avix-client-core --lib atp::types` |
| 5 | `cargo test -p avix-client-core --lib atp::event_emitter` |
