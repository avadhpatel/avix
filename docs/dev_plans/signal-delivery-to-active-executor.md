# Dev Plan: Signal Delivery to Active RuntimeExecutor Threads

**Status**: Draft  
**Created**: 2026-04-07  
**Tracks**: `docs/dev_plans/TODO.md` § "Signal Delivery to Active RuntimeExecutor Threads"

---

## Task Summary

Fix two distinct bugs in how signals reach a `RuntimeExecutor` that is blocked inside
an in-flight LLM call:

1. **Architecture bug** — The kernel's `SignalHandler` uses `SignalDelivery` to write
   signals to a per-agent Unix socket (`/run/avix/agents/<pid>.sock`).  The executor
   is supposed to listen on that socket via `start_signal_listener`.  In production
   (`IpcExecutorFactory::launch`) `start_signal_listener` is **never called**, so the
   socket never exists, so every kernel-side delivery logs a `warn` and silently fails.

2. **Timing bug** — Even if delivery worked, `run_with_client` never polls for incoming
   signals while `run_turn_streaming` is executing.  Signals are only visible *between*
   turns.  An LLM call can block for seconds to minutes.

---

## Architecture Spec References

- `docs/architecture/09-runtime-executor-tools.md`
- `docs/architecture/07-services.md` (pipe SIGPIPE delivery)
- `docs/architecture/00-overview.md` (signal table)

---

## Root Cause Analysis

| Component | Current state | Problem |
|-----------|--------------|---------|
| `SignalHandler` (`kernel/proc/signals.rs`) | Calls `SignalDelivery::deliver` → writes to `agents/<pid>.sock` | Socket never bound in production |
| `RuntimeExecutor::start_signal_listener` | Binds socket, spawns listener task | Never called from `IpcExecutorFactory::launch` |
| `RuntimeExecutor::deliver_signal` | Updates atomics only; docstring says "used in tests" | Not connected to `run_with_client` turn loop |
| `run_with_client` | Calls `run_turn_streaming` without any concurrent signal watch | Signals queued in atomics not seen until LLM returns |
| `pipe/manager.rs::close` | Takes `Option<&SignalDelivery>`, sends SIGPIPE via socket | Will also fail once socket is removed |

---

## Correct Design

```
Kernel (SignalHandler)
       │
       │  signal_channels.send(pid, signal)
       ▼
   SignalChannels
   Arc<Mutex<HashMap<u32, mpsc::Sender<Signal>>>>
       │
       │ mpsc channel
       ▼
RuntimeExecutor.signal_rx  ◄──── deliver_signal() also feeds this channel
       │
   tokio::select! in run_with_client
   ┌───────────────────────────┐
   │ llm_future (+ CancelToken)│   ◄── run_turn_streaming checks token
   │ signal_rx.recv()          │
   └───────────────────────────┘
```

- **`SignalChannels`** is a new `Arc<Mutex<HashMap<u32, mpsc::Sender<Signal>>>>` created
  once in `phase2_kernel` and shared between `IpcExecutorFactory` (registers at spawn,
  deregisters at exit) and `SignalHandler` (sends on it instead of the socket).
- **`RuntimeExecutor`** creates its own `mpsc` channel at construction, stores both
  `signal_tx` and `signal_rx`.  `deliver_signal` sends on `signal_tx` (kept for tests).
- **`run_with_client`** `.take()`s `signal_rx` at entry and uses `tokio::select!` to race
  the LLM future against incoming signals.
- **`run_turn_streaming`** accepts a `CancellationToken`; its inner stream loop checks
  `cancel.is_cancelled()` and breaks early if set.
- **`pipe/manager.rs::close`** switches from `Option<&SignalDelivery>` to
  `Option<&SignalChannels>` for SIGPIPE delivery.
- `signal/agent_socket.rs` and `signal/delivery.rs` are **deleted** (no more sockets on
  the agent side).

---

## Per-Signal Semantics During an Active LLM Call

| Signal | Action while `run_turn_streaming` is executing |
|--------|------------------------------------------------|
| `SIGKILL` | Cancel token → LLM future drops; finalize invocation as `Killed`; return `Err` from `run_with_client` |
| `SIGSTOP` | Cancel token; set `killed = true`; same as SIGKILL exit path |
| `SIGPAUSE` | Cancel token; set `paused = true`; return a new `Err(Paused)` variant so the caller can re-queue |
| `SIGPIPE` | Do NOT cancel; push payload as pending message; continue |
| `SIGSAVE` | Do NOT cancel; take interim snapshot; continue |
| `SIGRESUME` | Do NOT cancel; clear `paused`; continue |
| `SIGESCALATE` | Do NOT cancel; push HIL result as pending message; continue (full HIL wiring deferred) |

---

## Files to Change / Create / Delete

### Delete
| File | Why |
|------|-----|
| `crates/avix-core/src/signal/agent_socket.rs` | Per-agent socket approach removed entirely |
| `crates/avix-core/src/signal/delivery.rs` | Socket-based delivery removed; replaced by SignalChannels |

### Create
| File | What |
|------|------|
| `crates/avix-core/src/signal/channels.rs` | `SignalChannels` type alias + `SignalChannelRegistry` newtype with `register`, `unregister`, `send` methods |
| `crates/avix-core/tests/signal_interruption.rs` | Integration tests: SIGKILL / SIGPAUSE / SIGPIPE during simulated LLM call |

### Modify (in implementation order)

| # | File | Change summary |
|---|------|---------------|
| 1 | `crates/avix-core/Cargo.toml` | Add `tokio-util = { workspace = true, features = ["rt"] }` (for `CancellationToken`) |
| 2 | `Cargo.toml` (workspace) | Add `tokio-util` to `[workspace.dependencies]` if not present |
| 3 | `crates/avix-core/src/signal/channels.rs` | New: `SignalChannelRegistry` wrapping `Arc<Mutex<HashMap<u32, mpsc::Sender<Signal>>>>` with `register(pid, tx)`, `unregister(pid)`, `send(pid, signal) -> bool` |
| 4 | `crates/avix-core/src/signal/mod.rs` | Add `pub mod channels; pub use channels::SignalChannelRegistry;`; remove `pub mod agent_socket;` and `pub use delivery::SignalDelivery;` |
| 5 | `crates/avix-core/src/executor/runtime_executor.rs` | See detail below |
| 6 | `crates/avix-core/src/kernel/proc/signals.rs` | Replace `SignalDelivery` with `SignalChannelRegistry`; remove `runtime_dir` field; update `pause_agent`, `resume_agent`, `send_signal` |
| 7 | `crates/avix-core/src/kernel/proc/mod.rs` | Add `signal_channels: SignalChannelRegistry` field to `ProcHandler`; thread through `new`, `new_with_factory`, `with_invocation_store`, `with_session_store` |
| 8 | `crates/avix-core/src/bootstrap/executor_factory.rs` | Add `signal_channels: SignalChannelRegistry` field; register channel at launch, deregister at task exit |
| 9 | `crates/avix-core/src/bootstrap/mod.rs` | Create `SignalChannelRegistry` in `phase2_kernel`; pass to both `IpcExecutorFactory` and `ProcHandler::new_with_factory` |
| 10 | `crates/avix-core/src/pipe/manager.rs` | Replace `Option<&SignalDelivery>` with `Option<&SignalChannelRegistry>` in `close` and `close_pipes_for_pid`; use `registry.send(partner_pid, signal)` |
| 11 | `crates/avix-core/tests/signal_delivery.rs` | Remove socket-based tests (T-C-06, T-C-07, T-C-08, T-C-09); rewrite T-C-06/07 using `deliver_signal` directly; remove `agent_socket` imports |
| 12 | `crates/avix-core/tests/pipe_tools.rs` | Replace `SignalDelivery` with `SignalChannelRegistry`; register a test executor's signal_tx before calling `close` |
| 13 | `crates/avix-core/tests/signal_interruption.rs` | New integration tests (see below) |

---

## File-by-File Implementation Detail

### 1 & 2 — Cargo.toml

```toml
# workspace Cargo.toml — [workspace.dependencies]
tokio-util = { version = "0.7", features = ["rt", "sync"] }

# crates/avix-core/Cargo.toml — [dependencies]
tokio-util.workspace = true
```

---

### 3 — `signal/channels.rs` (NEW)

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use crate::signal::kind::Signal;
use crate::types::Pid;

/// In-process registry mapping agent PIDs to their signal mpsc senders.
///
/// `IpcExecutorFactory` registers a sender at spawn and deregisters at exit.
/// `SignalHandler` calls `send` instead of delivering over a socket.
#[derive(Clone, Default)]
pub struct SignalChannelRegistry {
    inner: Arc<Mutex<HashMap<u32, mpsc::Sender<Signal>>>>,
}

impl SignalChannelRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register the sender for `pid`.  Overwrites any previous registration.
    pub async fn register(&self, pid: Pid, tx: mpsc::Sender<Signal>) {
        self.inner.lock().await.insert(pid.as_u32(), tx);
    }

    /// Deregister a previously registered PID.
    pub async fn unregister(&self, pid: Pid) {
        self.inner.lock().await.remove(&pid.as_u32());
    }

    /// Send a signal to the registered executor for `pid`.
    /// Returns `true` if the channel existed, `false` if not registered.
    pub async fn send(&self, pid: Pid, signal: Signal) -> bool {
        let guard = self.inner.lock().await;
        if let Some(tx) = guard.get(&pid.as_u32()) {
            tx.send(signal).await.is_ok()
        } else {
            false
        }
    }
}
```

Unit tests: `register_and_send_reaches_receiver`, `send_returns_false_for_unknown_pid`,
`unregister_removes_entry`.

---

### 5 — `executor/runtime_executor.rs`

**Add to struct fields:**
```rust
/// Sender half of the in-process signal channel.
/// External callers (kernel, pipes) call `deliver_signal` which sends here.
signal_tx: tokio::sync::mpsc::Sender<Signal>,
/// Receiver half — taken once by `run_with_client` via `Option::take`.
signal_rx: Option<tokio::sync::mpsc::Receiver<Signal>>,
```

**Constructor** (`spawn_with_registry` / `spawn_with_registry_and_kernel`):
```rust
let (signal_tx, signal_rx) = tokio::sync::mpsc::channel(64);
// ... set on struct
```

**Modify `deliver_signal`** (keep `&self`, use `try_send` to avoid blocking):
```rust
pub async fn deliver_signal(&self, signal: &str) {
    // existing atomic updates ...

    // Also forward to the turn loop via the channel (best-effort; ignore if full).
    let sig = Signal {
        target: self.pid,
        kind: SignalKind::from_str(signal).unwrap_or(SignalKind::Usr1),
        payload: serde_json::Value::Null,
    };
    let _ = self.signal_tx.try_send(sig);
}
```

**Add `signal_sender` accessor** (for `IpcExecutorFactory` to clone and register):
```rust
pub fn signal_sender(&self) -> tokio::sync::mpsc::Sender<Signal> {
    self.signal_tx.clone()
}
```

**Modify `run_turn_streaming`** — add `cancel: tokio_util::sync::CancellationToken` parameter,
check it in the `tokio::select!` loop:
```rust
async fn run_turn_streaming(
    &self,
    req: LlmCompleteRequest,
    client: &dyn LlmClient,
    turn_id: Uuid,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<LlmCompleteResponse, AvixError> {
    // ... existing loop changes to:
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(AvixError::Cancelled("LLM call cancelled by signal".into()));
            }
            chunk_opt = stream.next() => { /* existing */ }
            _ = flush_timer.tick() => { flush_pending!(); }
        }
    }
    // ...
}
```

**Modify `run_with_client`** — take `signal_rx`, wrap LLM call in select:
```rust
pub async fn run_with_client(
    &mut self,
    goal: &str,
    client: &dyn LlmClient,
) -> Result<TurnResult, AvixError> {
    // Take the receiver once; if already taken (reentrant call in tests), create a
    // dummy channel whose receiver never yields.
    let mut signal_rx = self.signal_rx.take().unwrap_or_else(|| {
        let (_, rx) = tokio::sync::mpsc::channel(1);
        rx
    });

    // ... existing setup ...

    loop {
        // ... existing turn_id, req construction ...

        let cancel = tokio_util::sync::CancellationToken::new();
        let llm_fut = self.run_turn_streaming(req, client, turn_id, cancel.clone());

        let response = tokio::select! {
            res = llm_fut => {
                match res {
                    Ok(r) => r,
                    Err(AvixError::Cancelled(_)) => {
                        // Signal already handled below; loop exits via killed/paused flag
                        return Err(AvixError::Cancelled("agent stopped".into()));
                    }
                    Err(e) => return Err(e),
                }
            }
            Some(sig) = signal_rx.recv() => {
                cancel.cancel();
                self.handle_signal_during_llm(&sig).await;
                if self.killed.load(Ordering::Acquire) {
                    return Err(AvixError::Cancelled("SIGKILL".into()));
                }
                // SIGPAUSE: re-enqueue signal for next turn check; loop will see paused flag
                continue;
            }
        };

        // Between-turn signal check (existing signals that arrived via atomics only)
        if self.killed.load(Ordering::Acquire) {
            return Err(AvixError::Cancelled("killed".into()));
        }
        // ... rest of existing loop ...
    }
}
```

**Add private `handle_signal_during_llm`**:
```rust
async fn handle_signal_during_llm(&mut self, signal: &Signal) {
    let name = signal.kind.as_str();
    // Update atomics (reuse deliver_signal logic without re-sending to channel)
    match name {
        "SIGKILL" | "SIGSTOP" => {
            self.auto_log_session_end().await;
            self.killed.store(true, Ordering::Release);
            if let Some(vfs) = &self.vfs { self.write_status_yaml(vfs).await; }
        }
        "SIGPAUSE" => {
            self.paused.store(true, Ordering::Release);
            // update invocation + vfs (same as deliver_signal SIGPAUSE branch)
        }
        "SIGPIPE" => {
            // Enqueue pipe payload as a pending message for the next turn
            let text = signal.payload["text"].as_str().unwrap_or("[pipe data]");
            self.inject_pending_message(format!("[SIGPIPE]: {text}"));
        }
        "SIGSAVE" => {
            self.capture_and_write_snapshot(SnapshotTrigger::Sigsave, CapturedBy::Kernel).await;
            self.take_interim_snapshot().await;
        }
        _ => {
            tracing::debug!(pid = self.pid.as_u32(), signal = name, "unhandled signal during LLM");
        }
    }
}
```

**Remove** `start_signal_listener` method entirely.

---

### 6 — `kernel/proc/signals.rs`

Replace `SignalDelivery` with `SignalChannelRegistry`:

```rust
pub struct SignalHandler {
    signal_channels: SignalChannelRegistry,   // NEW — replaces runtime_dir
    process_table: Arc<ProcessTable>,
    // ... same other fields
}

impl SignalHandler {
    pub fn new(
        signal_channels: SignalChannelRegistry,
        process_table: Arc<ProcessTable>,
        // ...
    ) -> Self { ... }

    pub async fn pause_agent(&self, pid: u32) -> Result<(), AvixError> {
        // ... existing process_table + invocation_store updates ...

        // Replace SignalDelivery block with:
        let signal = Signal { target: Pid::new(pid), kind: SignalKind::Pause, payload: json!({}) };
        if !self.signal_channels.send(Pid::new(pid), signal).await {
            warn!(pid, "no signal channel registered for agent (not yet running?)");
        }

        // ... session cascade: use signal_channels.send for each sibling ...
        Ok(())
    }

    // resume_agent and send_signal: same pattern
}
```

---

### 7 — `kernel/proc/mod.rs`

Add `signal_channels: SignalChannelRegistry` field to `ProcHandler`.

Update `new`, `new_with_factory` to accept or create a `SignalChannelRegistry` and pass it to `SignalHandler::new`.

Update `with_invocation_store` and `with_session_store` (which currently rebuild `SignalHandler`) to pass `self.signal_channels.clone()`.

Add `with_signal_channels(mut self, sc: SignalChannelRegistry) -> Self` builder method.

---

### 8 — `bootstrap/executor_factory.rs`

```rust
pub struct IpcExecutorFactory {
    // existing fields ...
    signal_channels: SignalChannelRegistry,
}

impl IpcExecutorFactory {
    pub fn with_signal_channels(mut self, sc: SignalChannelRegistry) -> Self {
        self.signal_channels = sc; self
    }
}

impl AgentExecutorFactory for IpcExecutorFactory {
    fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle {
        let signal_channels = self.signal_channels.clone();
        let pid = params.pid;

        let handle = tokio::spawn(async move {
            // After executor is built:
            signal_channels.register(pid, executor.signal_sender()).await;

            // ... run_with_client ...

            // On any exit path:
            signal_channels.unregister(pid).await;
        });
        handle.abort_handle()
    }
}
```

---

### 9 — `bootstrap/mod.rs`

In `phase2_kernel`:
```rust
let signal_channels = SignalChannelRegistry::new();

let factory = Arc::new(
    IpcExecutorFactory::new(...)
        .with_signal_channels(signal_channels.clone())
        .with_tracer(...),
);

let proc_handler = Arc::new(
    ProcHandler::new_with_factory(...)
        .with_signal_channels(signal_channels)
        .with_manifest_scanner(...)
        .with_tracer(...)
        .with_invocation_store(...)
        .with_session_store(...),
);
```

---

### 10 — `pipe/manager.rs`

Change signatures:
```rust
pub async fn close(
    &self,
    pipe_id: &str,
    caller_pid: Pid,
    signal_channels: Option<&SignalChannelRegistry>,  // was: Option<&SignalDelivery>
    vfs: Option<&VfsRouter>,
    close_reason: &str,
) -> Result<(), AvixError> {
    // ...
    if let (Some(channels), Some(partner_pid)) = (signal_channels, partner) {
        let signal = Signal { target: partner_pid, kind: SignalKind::Pipe,
            payload: json!({ "pipeId": pipe_id, "reason": close_reason }) };
        channels.send(partner_pid, signal).await;
    }
    Ok(())
}

pub async fn close_pipes_for_pid(
    &self,
    pid: Pid,
    signal_channels: Option<&SignalChannelRegistry>,
    vfs: Option<&VfsRouter>,
) -> Result<(), AvixError> { /* delegate to close */ }
```

Update all callers of `close` / `close_pipes_for_pid` throughout the codebase (find via LSP `find_referencing_symbols`).

---

### 11 — `tests/signal_delivery.rs`

**Remove** tests T-C-06 (`agent_pause_resume_via_signal`), T-C-07 (`agent_kill_sets_killed_flag`),
T-C-08 (`agent_socket_created_and_removed`), T-C-09 (`remove_agent_socket_noop_when_missing`).

**Remove** `agent_socket`, `delivery` imports.

**Rewrite** equivalent tests to call `executor.deliver_signal("SIGPAUSE")` directly (no socket):
```rust
#[tokio::test]
async fn deliver_signal_pause_sets_paused_flag() {
    let executor = make_test_executor().await;
    executor.deliver_signal("SIGPAUSE").await;
    assert!(executor.paused.load(Ordering::Acquire));
}
```

Keep T-C-01 (`signal_kind_usr1_usr2_exist`) — no socket dependency.

---

### 12 — `tests/pipe_tools.rs`

Replace `SignalDelivery::new(...)` with `SignalChannelRegistry::new()`.
Register a mock signal receiver before calling `close`:
```rust
let registry = SignalChannelRegistry::new();
let (tx, mut rx) = tokio::sync::mpsc::channel(8);
registry.register(partner_pid, tx).await;

pipe_manager.close(&pipe_id, caller_pid, Some(&registry), None, "done").await.unwrap();

let sig = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
    .expect("no signal received")
    .expect("channel closed");
assert_eq!(sig.kind, SignalKind::Pipe);
```

---

### 13 — `tests/signal_interruption.rs` (NEW)

Key tests:

```
sigkill_during_llm_call_resolves_within_200ms
  — Mock LLM client that blocks for 2 s; deliver SIGKILL after 50 ms;
    assert run_with_client returns Err within 200 ms.

sigpause_during_llm_call_cancels_and_sets_paused
  — Deliver SIGPAUSE; assert paused flag set and run_with_client returns early.

sigpipe_during_llm_call_does_not_cancel
  — Deliver SIGPIPE while LLM "runs" (fast mock); assert LLM result is returned
    and pending_messages contains the pipe data.

sigsave_during_llm_call_takes_snapshot_and_continues
  — Deliver SIGSAVE; assert snapshot written; LLM call completes normally.
```

Use a `SlowMockLlmClient` that sleeps for a configurable duration, implementing
`LlmClient` (already exists or easy to add in the test module).

---

## Testing Strategy

```bash
# After each file (targeted):
cargo test --package avix-core signal::channels
cargo test --package avix-core executor::runtime_executor
cargo test --package avix-core kernel::proc::signals
cargo clippy --package avix-core -- -D warnings

# After all files:
cargo test --package avix-core --test signal_delivery
cargo test --package avix-core --test signal_interruption
cargo test --package avix-core --test pipe_tools
cargo test --package avix-core --test agent_lifecycle
```

Target: 95%+ on all touched modules.

---

## Dependencies

- `tokio-util 0.7` — `CancellationToken` in `tokio_util::sync`
- All other dependencies already in `Cargo.toml`

No new external crates beyond `tokio-util`.

---

## Architecture Spec Updates (after Mode 2 complete)

Update `docs/architecture/09-runtime-executor-tools.md`:
- Document `SignalChannelRegistry` as the canonical signal delivery path
- Document `run_with_client` select semantics
- Document per-signal behaviour during an active LLM call
- Remove mention of per-agent socket delivery
