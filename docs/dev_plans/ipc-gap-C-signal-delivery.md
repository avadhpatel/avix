# IPC Gap C — Signal Delivery Over IPC

> **Status:** Not started
> **Priority:** High — agents cannot be paused, killed, or controlled without this
> **Affects:** `avix-core/src/signal/`, new `avix-core/src/signal/delivery.rs`

---

## Problem

`signal/bus.rs` has a working in-memory `SignalBus` that can `send()` and `broadcast()` signals, and `signal/kind.rs` defines all signal types. However:

1. **No delivery mechanism over IPC.** The spec (`ipc-protocol.md §7`) requires signals to be delivered as JSON-RPC notifications (no `id` field) to a per-agent socket at `/run/avix/agents/<pid>.sock`. Nothing writes to those sockets.
2. **No signal receiver in agents.** `RuntimeExecutor` has no loop that reads from its agent socket and processes inbound signals.
3. **Agent-to-kernel signals** (`SIGUSR1`, `SIGUSR2`, `SIGESCALATE`) have no path from agent to kernel. `SIGESCALATE` (Human-in-Loop) is defined but triggers nothing.
4. **Agent socket lifecycle** is unmanaged — sockets are not created at spawn or removed at exit.

The signal spec (`ipc-signal.md`) additionally defines `SIGUSR1` and `SIGUSR2` (agent → kernel), which are absent from `SignalKind`.

---

## What Needs to Be Built

### 1. Complete `SignalKind` (`signal/kind.rs`)

Add the two missing variants from `ipc-signal.md`:

```rust
pub enum SignalKind {
    // existing ...
    Usr1,      // SIGUSR1 — agent-defined event, agent → kernel
    Usr2,      // SIGUSR2 — secondary agent-defined event, agent → kernel
}
```

Update `as_str()` accordingly.

### 2. Signal Delivery Service (`signal/delivery.rs`)

```rust
pub struct SignalDelivery {
    run_dir: PathBuf,
}

impl SignalDelivery {
    pub fn new(run_dir: PathBuf) -> Self;

    /// Deliver a signal to a specific agent's IPC socket.
    /// Sends a JSON-RPC notification (no `id`) to /run/avix/agents/<pid>.sock.
    pub async fn deliver(&self, signal: Signal) -> Result<(), AvixError>;

    /// Broadcast a signal to all agents in the provided pid list.
    pub async fn broadcast(&self, pids: &[Pid], kind: SignalKind, payload: serde_json::Value)
        -> Vec<(Pid, Result<(), AvixError>)>;
}
```

Wire format for delivery (per spec §7):

```json
{
  "jsonrpc": "2.0",
  "method": "signal",
  "params": {
    "signal": "SIGPAUSE",
    "payload": { "hilRequestId": "hil-001", "pendingTool": "email" }
  }
}
```

Uses `IpcClient::notify()` (from Gap A) — fire-and-forget, no response expected.

Errors:
- Socket not found → `AvixError::Enoent` (agent may not be running)
- Write fails → `AvixError::Eunavail` (agent may have crashed)
- Do NOT retry — signals are best-effort delivery

### 3. Agent Socket Lifecycle (`signal/agent_socket.rs`)

```rust
/// Create the agent's inbound signal socket at /run/avix/agents/<pid>.sock.
/// Called by RuntimeExecutor at spawn, before entering the LLM loop.
pub async fn create_agent_socket(run_dir: &Path, pid: Pid) -> Result<IpcServer, AvixError>;

/// Remove the agent's socket file. Called by RuntimeExecutor at shutdown.
pub async fn remove_agent_socket(run_dir: &Path, pid: Pid) -> Result<(), AvixError>;
```

The returned `IpcServer` is bound but not yet serving. The caller starts `serve()` with a signal handler.

### 4. Agent Signal Receiver

`RuntimeExecutor` needs a background task that accepts signals on its socket and acts on them. Add:

```rust
impl RuntimeExecutor {
    /// Start the agent signal socket listener.
    /// Returns a CancellationToken to stop it at shutdown.
    pub async fn start_signal_listener(
        &self,
        run_dir: &Path,
    ) -> Result<(JoinHandle<()>, CancellationToken), AvixError>;
}
```

The background task:
1. Creates agent socket at `run_dir/agents/<pid>.sock`
2. Serves with a handler that receives `JsonRpcRequest` (the notification)
3. Parses `params.signal` into `SignalKind`
4. Acts on signal:
   - `SIGPAUSE` → set `paused` atomic flag; the LLM loop checks this flag at each tool boundary
   - `SIGRESUME` → clear `paused` flag; wake any waiting task
   - `SIGKILL` → cancel the executor's `CancellationToken`; immediate exit
   - `SIGSTOP` → set `stopping` flag; LLM loop finishes current turn then exits cleanly
   - `SIGSAVE` → trigger snapshot (stub: log `info!("SIGSAVE received, snapshot not yet implemented")`)
   - `SIGPIPE` → log pipe closure, update pipe state (stub for now)
   - `SIGSTART` → no-op (agent is already running when it receives this)
5. Returns `None` (notifications never send a response)

### 5. Agent → Kernel Signal Path

For `SIGESCALATE`, `SIGUSR1`, `SIGUSR2` (agent → kernel direction):

```rust
impl RuntimeExecutor {
    /// Send a signal from this agent to the kernel.
    pub async fn send_signal_to_kernel(
        &self,
        kind: SignalKind,
        payload: serde_json::Value,
        run_dir: &Path,
    ) -> Result<(), AvixError>;
}
```

Uses `IpcClient::notify()` targeting `AVIX_KERNEL_SOCK`. The kernel's IPC server (not yet built — stub for now) will eventually route this to the appropriate handler.

`SIGESCALATE` is the primary use case: the agent calls this when it needs human approval. For now, log `info!("SIGESCALATE from pid={}", self.pid)` until the HIL subsystem is implemented.

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/signal_delivery.rs`.

```rust
// T-C-01: SignalKind includes SIGUSR1 and SIGUSR2
#[test]
fn signal_kind_usr1_usr2_exist() {
    assert_eq!(SignalKind::Usr1.as_str(), "SIGUSR1");
    assert_eq!(SignalKind::Usr2.as_str(), "SIGUSR2");
}

// T-C-02: Deliver sends notification to agent socket
#[tokio::test]
async fn deliver_sends_notification_to_agent_socket() {
    let dir = tempfile::tempdir().unwrap();
    // bind a mock server on agents/57.sock
    let received = Arc::new(Mutex::new(None));
    let received_clone = received.clone();
    // start mock server that records first notification
    let delivery = SignalDelivery::new(dir.path().to_owned());
    let sig = Signal { target: Pid::from(57), kind: SignalKind::Pause, payload: json!({}) };
    delivery.deliver(sig).await.unwrap();
    // assert received notification contains "SIGPAUSE"
}

// T-C-03: Deliver returns Enoent if socket does not exist
#[tokio::test]
async fn deliver_returns_enoent_for_missing_agent() {
    let dir = tempfile::tempdir().unwrap();
    let delivery = SignalDelivery::new(dir.path().to_owned());
    let sig = Signal { target: Pid::from(99), kind: SignalKind::Kill, payload: json!({}) };
    assert!(matches!(delivery.deliver(sig).await, Err(AvixError::Enoent(_))));
}

// T-C-04: Broadcast delivers to all listed PIDs
#[tokio::test]
async fn broadcast_reaches_multiple_agents() {
    // bind mock servers on agents/1.sock, agents/2.sock, agents/3.sock
    // broadcast to [1, 2, 3]
    // assert all three received the notification
}

// T-C-05: Broadcast tolerates missing agents (partial delivery)
#[tokio::test]
async fn broadcast_tolerates_missing_agents() {
    // only agents/1.sock exists; 2 and 3 do not
    // broadcast to [1, 2, 3]
    // assert pid=1 ok, pid=2 Enoent, pid=3 Enoent
    // does not panic or short-circuit
}

// T-C-06: Agent signal listener handles SIGPAUSE and SIGRESUME
#[tokio::test]
async fn agent_pause_resume_via_signal() {
    // create RuntimeExecutor (with mock registry)
    // start signal listener
    // deliver SIGPAUSE → assert paused flag is set
    // deliver SIGRESUME → assert paused flag is cleared
}

// T-C-07: Agent signal listener handles SIGKILL
#[tokio::test]
async fn agent_kill_cancels_executor() {
    // create RuntimeExecutor
    // start signal listener
    // deliver SIGKILL
    // assert executor CancellationToken is cancelled within 100ms
}

// T-C-08: Agent socket is removed at shutdown
#[tokio::test]
async fn agent_socket_removed_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    create_agent_socket(dir.path(), Pid::from(42)).await.unwrap();
    assert!(dir.path().join("agents/42.sock").exists());
    remove_agent_socket(dir.path(), Pid::from(42)).await.unwrap();
    assert!(!dir.path().join("agents/42.sock").exists());
}
```

---

## Implementation Notes

- The `paused` and `stopping` flags on `RuntimeExecutor` should be `Arc<AtomicBool>` shared between the executor and the signal listener task
- The LLM loop must check `paused.load(Ordering::Acquire)` at every tool-call boundary (before dispatching a tool). If paused, `tokio::time::sleep(10ms)` in a loop until resumed or killed
- Use `CancellationToken` from `tokio_util` crate for clean shutdown
- Agent socket creation must succeed before `ipc.register` is called (per spec §4, step 1)
- `broadcast()` should run all deliveries concurrently via `futures::future::join_all`
- Do not block signal listener on slow agent code — the listener task must remain responsive

---

## Success Criteria

- [ ] `SignalKind::Usr1` and `SignalKind::Usr2` added
- [ ] `SignalDelivery::deliver` and `broadcast` implemented and tested
- [ ] Agent socket created at spawn, removed at shutdown
- [ ] `RuntimeExecutor` background signal listener handles SIGPAUSE/SIGRESUME/SIGKILL/SIGSTOP
- [ ] All T-C-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes (no regressions)
