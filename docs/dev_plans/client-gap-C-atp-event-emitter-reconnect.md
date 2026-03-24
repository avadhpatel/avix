# Client Gap C — ATP Event Emitter + Reconnect Logic

> **Status:** Pending
> **Priority:** High
> **Depends on:** Client gap B (Dispatcher + AtpClient)
> **Blocks:** Client gaps D, G, H
> **Affects:** `crates/avix-client-core/src/atp/event_emitter.rs`

---

## Problem

The `Dispatcher` in gap B broadcasts raw `Event` frames on a channel, but there is no
typed fan-out system and no reconnect. When the server restarts or the socket drops,
clients need to reconnect automatically (60-second grace window, exponential backoff)
and re-subscribe without losing the broadcast channel endpoint.

---

## Scope

Implement `EventEmitter`, a typed fan-out layer that sits above `Dispatcher` and handles
reconnection. Consumers subscribe to specific `EventKind` variants; the emitter filters
and routes. When the underlying WS connection drops, the emitter reconnects using
exponential backoff capped at 60 seconds and re-establishes subscriptions transparently.

---

## What Needs to Be Built

### 1. `atp/event_emitter.rs`

```rust
use crate::atp::types::{Event, EventKind};
use crate::error::ClientError;
use tokio::sync::broadcast;

/// Receives ATP events, filters by kind, and fans out to typed subscribers.
pub struct EventEmitter {
    // Internally holds a broadcast::Sender<Event>
    // and the reconnect task handle.
}

impl EventEmitter {
    /// Create an emitter and start the reconnect-aware reader loop.
    /// `connect_fn` is a closure that returns a new `Dispatcher` — this lets
    /// the emitter reconnect without holding credentials itself.
    pub fn start<F, Fut>(connect_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<crate::atp::dispatcher::Dispatcher, ClientError>>
            + Send + 'static,
    { … }

    /// Subscribe to all events.
    pub fn subscribe_all(&self) -> broadcast::Receiver<Event> { … }

    /// Subscribe to a specific event kind only.
    pub fn subscribe(&self, kind: EventKind) -> broadcast::Receiver<Event> { … }

    /// True if currently connected.
    pub fn is_connected(&self) -> bool { … }
}
```

#### Reconnect loop (run inside `tokio::spawn`):

```
loop:
  attempt = connect_fn().await
  if ok:
    connected.store(true)
    forward events from dispatcher.events() → broadcast_sender
    on channel close / error: connected.store(false)
  else:
    wait backoff (1s → 2s → 4s … capped at 60s)
```

Use `tokio::time::sleep` for backoff. Reset backoff to 1s after a successful connection
that lasts at least 5 seconds (stable connection heuristic).

#### Filtered subscriber:

Spawn a small task that reads from `subscribe_all()` and only forwards matching kinds
into a fresh `broadcast::Sender<Event>`. The `subscribe(kind)` method returns the
receiver for that filtered channel.

---

## Reconnect Backoff Specification

| Attempt | Wait |
|---------|------|
| 1 | 1 s |
| 2 | 2 s |
| 3 | 4 s |
| 4 | 8 s |
| 5 | 16 s |
| 6+ | 60 s (capped) |

---

## Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::EventKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn subscribe_all_receives_forwarded_events() {
        // Arrange: build a fake connect_fn that returns a Dispatcher
        //          backed by an in-memory channel (from gap B test helpers).
        // Act: inject AgentOutput event into fake dispatcher.
        // Assert: subscribe_all() receiver gets the event.
    }

    #[tokio::test]
    async fn subscribe_kind_filters_correctly() {
        // Inject AgentOutput and SysAlert events.
        // Assert: subscribe(EventKind::SysAlert) receiver only gets SysAlert.
    }

    #[tokio::test]
    async fn reconnect_is_attempted_on_disconnect() {
        // Arrange: connect_fn returns error on first call, ok on second.
        // Use AtomicUsize to count calls.
        // Assert: connect_fn called twice; is_connected() becomes true.
    }

    #[tokio::test]
    async fn backoff_caps_at_60s() {
        // Unit test backoff calculation function directly (pure function).
        // Assert: backoff(6) == 60, backoff(10) == 60.
    }

    #[tokio::test]
    async fn is_connected_false_before_first_connection() {
        // immediately after EventEmitter::start with a failing connect_fn
        // is_connected() should be false
    }
}
```

---

## Implementation Notes

- `broadcast::channel` capacity: 256 (drop oldest on overflow — prefer that over blocking).
- The reconnect task holds a `JoinHandle`; drop it on `EventEmitter::drop` if needed.
- Log reconnect attempts at `tracing::warn!` and successful reconnects at `tracing::info!`.
- Never expose the raw `broadcast::Sender` publicly — only receivers.

---

## Success Criteria

- [ ] `EventEmitter::start` compiles and the reader loop runs in a background task
- [ ] `subscribe_all` / `subscribe(kind)` deliver the correct events in tests
- [ ] Reconnect is attempted on connection drop; backoff is capped at 60 s
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
