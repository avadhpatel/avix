# IPC Gap A — Real IPC Transport Layer

> **Status:** Not started
> **Priority:** Critical — blocks all other IPC gaps
> **Affects:** `avix-core/src/ipc/transport.rs`, new `avix-core/src/ipc/server.rs`, new `avix-core/src/ipc/client.rs`

---

## Problem

`ipc/transport.rs` contains only a test helper (`test_socket_pair`). There is no actual IPC server that accepts connections, dispatches JSON-RPC messages, or routes them to handlers. There is no client that opens connections to `AVIX_ROUTER_SOCK` / `AVIX_KERNEL_SOCK`. Everything currently uses in-memory mocks.

The spec (`ipc-protocol.md §2`) requires:
- Platform-resolved socket paths via env vars (`AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK`)
- A server that accepts one connection per call, dispatches to a handler, closes after response
- A client that opens a fresh connection per call, sends a request, reads the response, closes

---

## What Needs to Be Built

### 1. Platform Path Resolution (`ipc/platform.rs`)

```rust
pub fn kernel_sock_path(run_dir: &Path) -> PathBuf   // /run/avix/kernel.sock
pub fn router_sock_path(run_dir: &Path) -> PathBuf   // /run/avix/router.sock
pub fn agent_sock_path(run_dir: &Path, pid: Pid) -> PathBuf  // /run/avix/agents/<pid>.sock
pub fn svc_sock_path(run_dir: &Path, name: &str) -> PathBuf  // /run/avix/services/<name>.sock
```

- Linux/macOS: `UnixListener` paths under `/run/avix/`
- Windows: Named Pipes `\\.\pipe\avix-<name>` (stub — return `EUNAVAIL` for now)
- Resolve from env vars when available; fall back to computed default

### 2. IPC Server (`ipc/server.rs`)

A `IpcServer` that:
- Binds to a `PathBuf` socket path via `tokio::net::UnixListener`
- Exposes `serve(handler: H)` where `H: Fn(JsonRpcRequest) -> BoxFuture<JsonRpcResponse> + Send + Sync`
- Loops: `accept()` → `tokio::spawn(handle_connection(conn, handler))`
- Each spawned task: reads one frame, calls handler, writes response frame, closes connection
- Signal notifications (no `id` field): call handler but do not write a response frame
- Graceful shutdown via `CancellationToken`

```rust
pub struct IpcServer {
    path: PathBuf,
    cancel: CancellationToken,
}

impl IpcServer {
    pub async fn bind(path: PathBuf) -> Result<Self, AvixError>;
    pub async fn serve<H>(self, handler: H) -> Result<(), AvixError>
    where
        H: Fn(JsonRpcRequest) -> BoxFuture<'static, Option<JsonRpcResponse>> + Send + Sync + 'static;
    pub fn cancel(&self);
}
```

### 3. IPC Client (`ipc/client.rs`)

A `IpcClient` that:
- Accepts a target socket `PathBuf`
- Exposes `call(request: JsonRpcRequest) -> Result<JsonRpcResponse, AvixError>`
- Opens a fresh `UnixStream` per call, writes request frame, reads response frame, closes
- Applies configurable timeout (`DEFAULT_CALL_TIMEOUT_MS = 5000`)
- Maps transport errors to `JsonRpcErrorCode`

```rust
pub struct IpcClient {
    target: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    pub fn new(target: PathBuf) -> Self;
    pub fn with_timeout(self, d: Duration) -> Self;
    pub async fn call(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse, AvixError>;
    pub async fn notify(&self, req: JsonRpcRequest) -> Result<(), AvixError>;  // fire-and-forget notification
}
```

### 4. Update Transport Module

Replace `transport.rs` contents:
- Keep `test_socket_pair()` (it is used by frame tests)
- Re-export `IpcServer`, `IpcClient`, and platform path helpers

---

## TDD Test Plan

All tests go in `crates/avix-core/tests/ipc_transport.rs`.

```rust
// T-A-01: Server binds and accepts a single call
#[tokio::test]
async fn server_accepts_single_call() {
    // bind server on temp socket path
    // client calls server with method="ping"
    // handler returns ok result
    // assert client receives ok response
}

// T-A-02: Each connection is independent (concurrent calls)
#[tokio::test]
async fn server_handles_concurrent_calls() {
    // bind server
    // spawn 10 concurrent client calls
    // assert all receive valid responses
}

// T-A-03: Notification (no id) does not produce response
#[tokio::test]
async fn server_ignores_notification_response() {
    // send notification (no id field)
    // assert connection closes without response frame
}

// T-A-04: Client times out if server is slow
#[tokio::test]
async fn client_timeout_on_slow_server() {
    // handler sleeps 200ms
    // client has 50ms timeout
    // assert Etimeout error
}

// T-A-05: Client reconnects on fresh call after server restart
#[tokio::test]
async fn client_fresh_connection_per_call() {
    // call 1 succeeds
    // server is dropped and rebound on same path
    // call 2 succeeds
}

// T-A-06: Server graceful shutdown drains in-flight calls
#[tokio::test]
async fn server_graceful_shutdown() {
    // start long-running handler (100ms)
    // cancel token while call is in flight
    // assert in-flight call completes before server exits
}

// T-A-07: Frame size limit enforced
#[tokio::test]
async fn oversized_frame_rejected() {
    // client sends frame with payload > MAX_FRAME_BYTES
    // assert AvixError::FrameTooLarge
}
```

Performance target: IPC round-trip (local socket) < 500 µs (see CLAUDE.md).

---

## Implementation Notes

- Use `tokio::net::UnixListener` and `tokio::net::UnixStream` throughout
- `serve()` must tolerate `accept()` errors (log at `warn!`, continue loop)
- Socket file must be removed on bind if it already exists (stale from previous crash)
- Use `tokio::time::timeout` wrapping the entire `call()` in the client
- `notify()` opens a connection, writes the frame, does NOT read a response, closes
- Do not implement Windows Named Pipes now — stub with `cfg!(target_os = "windows")` returning `Err(AvixError::Unsupported)`

---

## Success Criteria

- [ ] `IpcServer::bind` + `serve` compiles and passes all T-A-* tests
- [ ] `IpcClient::call` and `notify` compile and pass all T-A-* tests
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] Round-trip latency ≤ 500 µs verified in a benchmark in `benches/ipc.rs`
- [ ] Frame encode+decode ≤ 10 µs (existing benchmark still passes)
