# IPC Gap E — Pipe IPC Tool Handlers

> **Status:** Not started
> **Priority:** Medium — required for multi-agent workflows
> **Depends on:** Gap A (IPC transport), Gap B (router dispatch)
> **Affects:** `avix-core/src/pipe/`, `avix-core/src/executor/tool_registration.rs`

---

## Problem

`pipe/channel.rs` and `pipe/registry.rs` implement an in-memory pipe with correct FIFO semantics and backpressure. However:

1. **No IPC tool handlers exist.** The Cat2 tools `pipe/open`, `pipe/write`, `pipe/read`, `pipe/close` are registered in `compute_cat2_tools()` and have descriptors in `cat2_tool_descriptor()`, but there are no handler functions that back them.
2. **Authorization is absent.** Any caller can read from or write to any pipe ID. The spec requires pipes to be scoped to their owning agent pair.
3. **VFS manifest missing.** The spec (`ipc-pipe.md`) states the kernel writes a `Pipe` record to `/proc/<sourcePid>/pipes/<pipeId>.yaml` when a pipe is created. This write never happens.
4. **Lifecycle enforcement absent.** When an agent exits, its pipes should be closed and `SIGPIPE` delivered to the partner agent. No cleanup hook exists.
5. **`direction` and `encoding` fields** from the schema are defined in the spec but not modelled in `Pipe` struct — the current implementation is always unidirectional source→target.

---

## What Needs to Be Built

### 1. Extend `Pipe` Struct (`pipe/channel.rs`)

Add fields to match the spec schema:

```rust
pub enum PipeDirection {
    Out,           // source → target only
    In,            // target → source only
    Bidirectional, // both directions
}

pub enum PipeEncoding {
    Text,   // UTF-8 strings (current behavior)
    Json,   // Validated JSON strings
    Yaml,   // Validated YAML strings
}

pub enum BackpressurePolicy {
    Block,   // write blocks until read (current `try_send` becomes `send().await`)
    Drop,    // excess tokens silently dropped
    Error,   // return PipeError::Full (current behavior for `try_send` failure)
}

pub struct PipeConfig {
    pub source_pid: Pid,
    pub target_pid: Pid,
    pub direction: PipeDirection,
    pub buffer_tokens: usize,      // default: 8192
    pub backpressure: BackpressurePolicy,  // default: Block
    pub encoding: PipeEncoding,    // default: Text
}
```

Adjust `Pipe::new` to accept `PipeConfig`. Fix `write()` to honor `BackpressurePolicy`:
- `Block` → `sender.send(msg).await` (blocks until buffer space)
- `Drop` → `sender.try_send(msg).ok()` (silently drops on full)
- `Error` → `sender.try_send(msg).map_err(|_| PipeError::Full(...))` (current behavior)

For `PipeEncoding::Json` / `Yaml`: validate payload before writing, return `PipeError::InvalidEncoding` if invalid.

### 2. Pipe Tool Handlers (`pipe/handlers.rs`)

```rust
/// pipe/open — Create a new pipe between two agents.
/// Only the source agent can open a pipe (caller must be source_pid).
pub async fn handle_pipe_open(
    params: PipeOpenParams,
    caller_pid: Pid,
    registry: Arc<RwLock<PipeRegistry>>,
    vfs: Arc<MemFs>,
) -> Result<serde_json::Value, AvixError>;

/// pipe/write — Write a message to a pipe.
/// Caller must be the source agent of this pipe.
pub async fn handle_pipe_write(
    params: PipeWriteParams,
    caller_pid: Pid,
    registry: Arc<RwLock<PipeRegistry>>,
) -> Result<serde_json::Value, AvixError>;

/// pipe/read — Read the next message from a pipe.
/// Caller must be the target agent of this pipe.
pub async fn handle_pipe_read(
    params: PipeReadParams,
    caller_pid: Pid,
    registry: Arc<RwLock<PipeRegistry>>,
) -> Result<serde_json::Value, AvixError>;

/// pipe/close — Close a pipe.
/// Either agent may close the pipe.
pub async fn handle_pipe_close(
    params: PipeCloseParams,
    caller_pid: Pid,
    registry: Arc<RwLock<PipeRegistry>>,
    signal_delivery: Arc<SignalDelivery>,
    process_table: Arc<RwLock<ProcessTable>>,
) -> Result<serde_json::Value, AvixError>;
```

Input param structs:

```rust
pub struct PipeOpenParams {
    pub target_pid: u32,
    pub direction: Option<String>,       // "out" | "in" | "bidirectional", default "out"
    pub buffer_tokens: Option<usize>,
    pub backpressure: Option<String>,    // "block" | "drop" | "error"
    pub encoding: Option<String>,        // "text" | "json" | "yaml"
}

pub struct PipeWriteParams {
    pub pipe_id: String,
    pub message: String,
}

pub struct PipeReadParams {
    pub pipe_id: String,
    pub timeout_ms: Option<u64>,         // default: 5000ms
}

pub struct PipeCloseParams {
    pub pipe_id: String,
}
```

Output shapes:

```rust
// pipe/open success:   { "pipe_id": "pipe-<ulid>" }
// pipe/write success:  { "ok": true }
// pipe/read success:   { "message": "..." }    OR  { "status": "closed" }  OR  { "status": "timeout" }
// pipe/close success:  { "ok": true }
```

### 3. Authorization in Handlers

The `PipeRegistry` must track `source_pid` and `target_pid` per pipe. Add to `PipeRegistry`:

```rust
pub struct PipeRecord {
    pub pipe: Arc<Pipe>,
    pub config: PipeConfig,
}

pub struct PipeRegistry {
    pipes: HashMap<String, PipeRecord>,  // pipe_id → record
}
```

Enforce in handlers:
- `pipe/write`: `caller_pid == config.source_pid` (or `bidirectional`: either pid)
- `pipe/read`: `caller_pid == config.target_pid` (or `bidirectional`: either pid)
- `pipe/close`: either pid may close
- `pipe/open`: `caller_pid == params` source (the caller *is* the source)

Return `EPERM` on authorization failure.

### 4. VFS Manifest Write (`pipe/vfs.rs`)

On `pipe/open` success, write to VFS:

```rust
pub async fn write_pipe_manifest(
    vfs: &MemFs,
    config: &PipeConfig,
    pipe_id: &str,
) -> Result<(), AvixError>;
```

Path: `/proc/<source_pid>/pipes/<pipe_id>.yaml`

YAML content (matches `ipc-pipe.md` schema):

```yaml
apiVersion: avix/v1
kind: Pipe
metadata:
  pipeId: <pipe_id>
  createdAt: <ISO8601>
  createdBy: kernel

spec:
  sourcePid: <source_pid>
  targetPid: <target_pid>
  direction: <direction>
  bufferTokens: <buffer_tokens>
  backpressure: <backpressure>
  encoding: <encoding>

status:
  state: open
  tokensSent: 0
  tokensAcknowledged: 0
  closedAt: null
  closedReason: null
```

Update the manifest's `status.state: closed` and `closedAt` when `pipe/close` is called.

### 5. Lifecycle Cleanup on Agent Exit

When a process exits (called by `RuntimeExecutor::shutdown()`), close all pipes owned by that PID and signal the partner:

```rust
pub async fn close_pipes_for_pid(
    pid: Pid,
    registry: Arc<RwLock<PipeRegistry>>,
    signal_delivery: Arc<SignalDelivery>,
    vfs: Arc<MemFs>,
) -> Result<(), AvixError>;
```

For each pipe where `source_pid == pid` or `target_pid == pid`:
1. Call `registry.close(pipe_id)`
2. Deliver `SIGPIPE` to the *other* agent
3. Update VFS manifest `status.state: closed`, `closedReason: "owner_exited"`

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/pipe_tools.rs`.

```rust
// T-E-01: pipe/open creates pipe and returns pipe_id
#[tokio::test]
async fn pipe_open_creates_pipe() {
    // caller_pid=10, target_pid=20
    // handle_pipe_open returns { pipe_id: "pipe-..." }
    // registry.pipe_count() == 1
}

// T-E-02: pipe/write succeeds for source agent
#[tokio::test]
async fn pipe_write_by_source_succeeds() {
    // open pipe (source=10, target=20)
    // write("hello") from pid=10 → ok
}

// T-E-03: pipe/write fails for non-source agent
#[tokio::test]
async fn pipe_write_by_non_source_fails() {
    // open pipe (source=10, target=20)
    // write from pid=30 → Err(Eperm)
}

// T-E-04: pipe/read returns message for target agent
#[tokio::test]
async fn pipe_read_by_target_succeeds() {
    // open pipe, write "hello" from source
    // read from pid=20 → { message: "hello" }
}

// T-E-05: pipe/read fails for non-target agent
#[tokio::test]
async fn pipe_read_by_non_target_fails() {
    // open pipe (source=10, target=20)
    // read from pid=10 (not target) → Err(Eperm)
}

// T-E-06: pipe/read times out when no message
#[tokio::test]
async fn pipe_read_times_out() {
    // open pipe, no write
    // read with timeout_ms=50 → { status: "timeout" }
}

// T-E-07: pipe/read returns closed when pipe is closed
#[tokio::test]
async fn pipe_read_returns_closed_after_close() {
    // open pipe, close it
    // read → { status: "closed" }
}

// T-E-08: pipe/close sends SIGPIPE to partner
#[tokio::test]
async fn pipe_close_delivers_sigpipe_to_partner() {
    // open pipe (source=10, target=20)
    // close from pid=10
    // assert SIGPIPE delivered to pid=20
}

// T-E-09: pipe/open writes VFS manifest
#[tokio::test]
async fn pipe_open_writes_vfs_manifest() {
    // open pipe
    // read vfs at /proc/10/pipes/<pipe_id>.yaml
    // assert YAML parses correctly with sourcePid=10, targetPid=20, state=open
}

// T-E-10: VFS manifest updated on close
#[tokio::test]
async fn pipe_close_updates_vfs_manifest() {
    // open, then close pipe
    // read VFS manifest
    // assert status.state == "closed"
}

// T-E-11: Backpressure=Drop silently drops on full buffer
#[tokio::test]
async fn backpressure_drop_discards_on_full() {
    // open pipe with buffer_tokens=2, backpressure=drop
    // write 3 messages from source
    // read: only 2 arrive (third was dropped)
}

// T-E-12: Backpressure=Error returns PipeError on full
#[tokio::test]
async fn backpressure_error_returns_error_on_full() {
    // open pipe with buffer_tokens=1, backpressure=error
    // fill buffer with 1 message
    // second write → Err(PipeError::Full)
}

// T-E-13: Agent exit closes all pipes
#[tokio::test]
async fn agent_exit_closes_owned_pipes() {
    // open 2 pipes from pid=10
    // call close_pipes_for_pid(10)
    // both pipes are closed in registry
    // SIGPIPE delivered to target pids
}

// T-E-14: Bidirectional pipe allows both agents to write
#[tokio::test]
async fn bidirectional_pipe_both_agents_write() {
    // open pipe (direction=bidirectional, source=10, target=20)
    // write from pid=10 → ok
    // write from pid=20 → ok
    // read from pid=10 gets message from 20
    // read from pid=20 gets message from 10
}
```

---

## Implementation Notes

- Use `ulid` for pipe IDs (`pipe-<ULID>`) consistent with job IDs in Gap D
- Bidirectional pipes require two MPSC channels (one per direction); the `Pipe` struct should hold both
- `pipe/read` timeout: use `tokio::time::timeout(timeout_ms, registry.read(id))`
- JSON encoding validation: use `serde_json::from_str::<serde_json::Value>` before writing; reject if parse fails
- Authorization check must be a fast path — do NOT hold `RwLock` while awaiting reads; grab config, drop lock, then await
- VFS writes for pipe manifests are in the `/proc/` tree — kernel-owned. The handlers are kernel-side code so this is allowed per architecture invariant §15/§16
- `close_pipes_for_pid` should be called from `RuntimeExecutor::shutdown()` *before* deregistering Cat2 tools

---

## Success Criteria

- [ ] `PipeConfig`, `PipeDirection`, `PipeEncoding`, `BackpressurePolicy` defined
- [ ] `PipeRegistry` stores `PipeRecord` with config and enforces authorization
- [ ] `handle_pipe_open`, `handle_pipe_write`, `handle_pipe_read`, `handle_pipe_close` implemented
- [ ] VFS manifest written on open, updated on close
- [ ] `SIGPIPE` delivered to partner on close (uses Gap C's `SignalDelivery`)
- [ ] `close_pipes_for_pid` called from `RuntimeExecutor::shutdown()`
- [ ] All T-E-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes (no regressions)
