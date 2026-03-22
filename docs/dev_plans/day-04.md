# Day 4 — SignalBus with TDD

> **Goal:** Build the in-memory signal bus — the mechanism through which the kernel delivers `SIGPAUSE`, `SIGRESUME`, `SIGKILL`, `SIGESCALATE`, `SIGSAVE`, and other signals to agents. Supports broadcast, targeted delivery, and multiple subscribers per PID.

---

## Pre-flight: Verify Day 3

```bash
cargo test --workspace
# Expected: all Day 3 tests pass (15+ process table tests)

# Confirm ProcessTable exists and is accessible
grep -r "pub struct ProcessTable" crates/avix-core/src/
grep -r "pub async fn insert"     crates/avix-core/src/process/

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings
```

All checks must pass before writing new code.

---

## Step 1 — Extend the Module Tree

Add to `crates/avix-core/src/lib.rs`:

```rust
pub mod error;
pub mod types;
pub mod process;
pub mod signal;   // NEW
```

Create `crates/avix-core/src/signal/mod.rs`:

```rust
pub mod bus;
pub mod kind;

pub use bus::SignalBus;
pub use kind::{Signal, SignalKind};
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/signal_bus.rs`:

```rust
use avix_core::signal::{Signal, SignalBus, SignalKind};
use avix_core::types::Pid;
use std::sync::Arc;
use std::time::Duration;

fn sigpause(pid: u32) -> Signal {
    Signal { target: Pid::new(pid), kind: SignalKind::Pause, payload: serde_json::Value::Null }
}

fn sigresume(pid: u32, payload: serde_json::Value) -> Signal {
    Signal { target: Pid::new(pid), kind: SignalKind::Resume, payload }
}

fn sigkill(pid: u32) -> Signal {
    Signal { target: Pid::new(pid), kind: SignalKind::Kill, payload: serde_json::Value::Null }
}

// ── Basic subscribe and receive ───────────────────────────────────────────────

#[tokio::test]
async fn subscribe_and_receive_signal() {
    let bus = SignalBus::new();
    let mut rx = bus.subscribe(Pid::new(57)).await;

    bus.send(sigpause(57)).await.unwrap();

    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("timeout")
        .expect("channel closed");

    assert_eq!(sig.kind, SignalKind::Pause);
    assert_eq!(sig.target, Pid::new(57));
}

// ── Multiple subscribers for the same PID ─────────────────────────────────────

#[tokio::test]
async fn multiple_subscribers_all_receive() {
    let bus = SignalBus::new();
    let mut rx1 = bus.subscribe(Pid::new(57)).await;
    let mut rx2 = bus.subscribe(Pid::new(57)).await;

    bus.send(sigpause(57)).await.unwrap();

    let s1 = tokio::time::timeout(Duration::from_millis(100), rx1.recv()).await.unwrap().unwrap();
    let s2 = tokio::time::timeout(Duration::from_millis(100), rx2.recv()).await.unwrap().unwrap();

    assert_eq!(s1.kind, SignalKind::Pause);
    assert_eq!(s2.kind, SignalKind::Pause);
}

// ── Signal is not delivered to wrong PID ──────────────────────────────────────

#[tokio::test]
async fn signal_not_delivered_to_wrong_pid() {
    let bus = SignalBus::new();
    let mut rx_57 = bus.subscribe(Pid::new(57)).await;
    let mut rx_58 = bus.subscribe(Pid::new(58)).await;

    bus.send(sigpause(57)).await.unwrap();

    // PID 57 receives it
    let s = tokio::time::timeout(Duration::from_millis(100), rx_57.recv()).await.unwrap().unwrap();
    assert_eq!(s.kind, SignalKind::Pause);

    // PID 58 does NOT receive it
    let nothing = tokio::time::timeout(Duration::from_millis(50), rx_58.recv()).await;
    assert!(nothing.is_err(), "PID 58 should not have received the signal");
}

// ── SIGRESUME with payload ─────────────────────────────────────────────────────

#[tokio::test]
async fn sigresume_carries_payload() {
    let bus = SignalBus::new();
    let mut rx = bus.subscribe(Pid::new(57)).await;

    let payload = serde_json::json!({ "hilId": "hil-001", "decision": "approved" });
    bus.send(sigresume(57, payload.clone())).await.unwrap();

    let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await.unwrap().unwrap();
    assert_eq!(sig.kind, SignalKind::Resume);
    assert_eq!(sig.payload["hilId"], "hil-001");
    assert_eq!(sig.payload["decision"], "approved");
}

// ── Broadcast to all PIDs ─────────────────────────────────────────────────────

#[tokio::test]
async fn broadcast_reaches_all_subscribers() {
    let bus = SignalBus::new();
    let mut rx57 = bus.subscribe(Pid::new(57)).await;
    let mut rx58 = bus.subscribe(Pid::new(58)).await;
    let mut rx59 = bus.subscribe(Pid::new(59)).await;

    bus.broadcast(SignalKind::Kill, serde_json::Value::Null).await;

    for rx in [&mut rx57, &mut rx58, &mut rx59] {
        let s = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await.unwrap().unwrap();
        assert_eq!(s.kind, SignalKind::Kill);
    }
}

// ── Unsubscribe cleans up ─────────────────────────────────────────────────────

#[tokio::test]
async fn unsubscribe_stops_delivery() {
    let bus = Arc::new(SignalBus::new());
    let rx = bus.subscribe(Pid::new(57)).await;
    let id = rx.id();

    bus.unsubscribe(Pid::new(57), id).await;
    bus.send(sigpause(57)).await.unwrap(); // Should not panic even with no receivers

    // subscriber count drops to 0
    assert_eq!(bus.subscriber_count(Pid::new(57)).await, 0);
}

// ── Send to PID with no subscribers is a no-op ───────────────────────────────

#[tokio::test]
async fn send_to_unsubscribed_pid_is_noop() {
    let bus = SignalBus::new();
    // PID 99 has no subscribers — must not return error or panic
    bus.send(sigpause(99)).await.unwrap();
}

// ── All signal kinds parse correctly ─────────────────────────────────────────

#[test]
fn signal_kind_names() {
    assert_eq!(SignalKind::Pause.as_str(),    "SIGPAUSE");
    assert_eq!(SignalKind::Resume.as_str(),   "SIGRESUME");
    assert_eq!(SignalKind::Kill.as_str(),     "SIGKILL");
    assert_eq!(SignalKind::Stop.as_str(),     "SIGSTOP");
    assert_eq!(SignalKind::Save.as_str(),     "SIGSAVE");
    assert_eq!(SignalKind::Escalate.as_str(), "SIGESCALATE");
    assert_eq!(SignalKind::Start.as_str(),    "SIGSTART");
    assert_eq!(SignalKind::Pipe.as_str(),     "SIGPIPE");
}

// ── Concurrency ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_sends_all_received() {
    let bus = Arc::new(SignalBus::new());
    let mut rx = bus.subscribe(Pid::new(57)).await;

    let mut senders = Vec::new();
    for _ in 0..20 {
        let b = Arc::clone(&bus);
        senders.push(tokio::spawn(async move {
            b.send(sigpause(57)).await.unwrap();
        }));
    }

    for s in senders { s.await.unwrap(); }

    let mut count = 0;
    while tokio::time::timeout(Duration::from_millis(50), rx.recv()).await.is_ok() {
        count += 1;
        if count == 20 { break; }
    }
    assert_eq!(count, 20);
}
```

---

## Step 3 — Implement

**`src/signal/kind.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SignalKind {
    Start,
    Pause,
    Resume,
    Kill,
    Stop,
    Save,
    Pipe,
    Escalate,
}

impl SignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Start    => "SIGSTART",
            Self::Pause    => "SIGPAUSE",
            Self::Resume   => "SIGRESUME",
            Self::Kill     => "SIGKILL",
            Self::Stop     => "SIGSTOP",
            Self::Save     => "SIGSAVE",
            Self::Pipe     => "SIGPIPE",
            Self::Escalate => "SIGESCALATE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub target:  crate::types::Pid,
    pub kind:    SignalKind,
    pub payload: serde_json::Value,
}
```

**`src/signal/bus.rs`**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use crate::types::Pid;
use super::kind::{Signal, SignalKind};

const CHANNEL_CAPACITY: usize = 64;

/// Unique handle identifying a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

pub struct Subscription {
    pub(crate) id: SubscriptionId,
    inner: broadcast::Receiver<Signal>,
}

impl Subscription {
    pub fn id(&self) -> SubscriptionId { self.id }

    pub async fn recv(&mut self) -> Option<Signal> {
        self.inner.recv().await.ok()
    }
}

struct PidEntry {
    sender: broadcast::Sender<Signal>,
    /// Track IDs so unsubscribe knows when to drop the sender.
    sub_count: usize,
}

#[derive(Default)]
pub struct SignalBus {
    table:   Arc<RwLock<HashMap<u32, PidEntry>>>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl SignalBus {
    pub fn new() -> Self { Self::default() }

    pub async fn subscribe(&self, pid: Pid) -> Subscription {
        let id = SubscriptionId(
            self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let mut table = self.table.write().await;
        let entry = table.entry(pid.as_u32()).or_insert_with(|| PidEntry {
            sender:    broadcast::channel(CHANNEL_CAPACITY).0,
            sub_count: 0,
        });
        entry.sub_count += 1;
        let rx = entry.sender.subscribe();
        Subscription { id, inner: rx }
    }

    pub async fn unsubscribe(&self, pid: Pid, _id: SubscriptionId) {
        let mut table = self.table.write().await;
        if let Some(entry) = table.get_mut(&pid.as_u32()) {
            entry.sub_count = entry.sub_count.saturating_sub(1);
            if entry.sub_count == 0 {
                table.remove(&pid.as_u32());
            }
        }
    }

    pub async fn send(&self, signal: Signal) -> Result<(), ()> {
        let table = self.table.read().await;
        if let Some(entry) = table.get(&signal.target.as_u32()) {
            let _ = entry.sender.send(signal); // ignore send error (no receivers = ok)
        }
        Ok(())
    }

    pub async fn broadcast(&self, kind: SignalKind, payload: serde_json::Value) {
        let table = self.table.read().await;
        for (pid_u32, entry) in table.iter() {
            let sig = Signal {
                target:  crate::types::Pid::new(*pid_u32),
                kind:    kind.clone(),
                payload: payload.clone(),
            };
            let _ = entry.sender.send(sig);
        }
    }

    pub async fn subscriber_count(&self, pid: Pid) -> usize {
        self.table.read().await
            .get(&pid.as_u32())
            .map(|e| e.sub_count)
            .unwrap_or(0)
    }
}
```

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: all Day 4 tests pass (10+ new signal bus tests)

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings

cargo fmt --check
# Expected: exit 0
```

---

## Commit

```bash
git add -A
git commit -m "day-04: SignalBus with broadcast, targeted delivery, all signal kinds"
```

---

## Success Criteria

- [ ] 10+ signal bus tests pass
- [ ] Single subscriber receives targeted signal
- [ ] Multiple subscribers on same PID all receive
- [ ] Signal not delivered to wrong PID
- [ ] `SIGRESUME` payload round-trips through the bus
- [ ] `broadcast` reaches all subscribed PIDs
- [ ] Send to unsubscribed PID is a no-op (no panic or error)
- [ ] All 8 `SignalKind` values have correct `as_str()` names
- [ ] Concurrent sends (20) all received
- [ ] 0 clippy warnings
