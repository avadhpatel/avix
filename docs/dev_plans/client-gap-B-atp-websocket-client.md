# Client Gap B — ATP WebSocket Client (Login + Connect + Send/Receive)

> **Status:** Pending
> **Priority:** Critical — all live-connection gaps depend on this
> **Depends on:** Client gap A (`avix-client-core` scaffold + ATP types)
> **Blocks:** Client gaps C, E, F, G, H
> **Affects:** `crates/avix-client-core/src/atp/client.rs`,
>   `crates/avix-client-core/src/atp/dispatcher.rs`

---

## Problem

There is no ATP client that can authenticate against a running `gateway.svc` and
exchange ATP frames over WebSocket. Both GUI and CLI need this capability; it must live
in `avix-client-core` so it is shared exactly once.

---

## Scope

Implement HTTP login (`POST /atp/auth/login`) and WebSocket upgrade with bearer auth,
plus a `Dispatcher` that correlates outgoing `Cmd` frames with incoming `Reply` frames
by `id`. No reconnect logic yet (gap C). No event fan-out yet (gap C).

---

## What Needs to Be Built

### 1. `atp/client.rs` — `AtpClient`

```rust
use crate::atp::types::{LoginRequest, LoginResponse, Cmd, Frame};
use crate::error::ClientError;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;

type WsSink = SplitSink<
    tokio_tungstenite::WebSocketStream<MaybeTlsStream<TcpStream>>,
    Message,
>;
type WsStream = SplitStream<
    tokio_tungstenite::WebSocketStream<MaybeTlsStream<TcpStream>>,
>;

pub struct AtpClient {
    pub session: LoginResponse,
    sink: WsSink,
    pub stream: WsStream,
}

impl AtpClient {
    /// Authenticate and open a WebSocket connection.
    /// `base_url` is the HTTP base, e.g. "http://127.0.0.1:7700".
    pub async fn connect(
        base_url: &str,
        identity: &str,
        credential: &str,
    ) -> Result<Self, ClientError> { … }

    /// Send a single `Cmd` frame.
    pub async fn send(&mut self, cmd: &Cmd) -> Result<(), ClientError> { … }

    /// Read the next raw frame from the stream.
    /// Returns `None` if the connection is closed.
    pub async fn next_frame(&mut self) -> Option<Result<Frame, ClientError>> { … }
}
```

#### Implementation notes for `connect`:

1. `POST {base_url}/atp/auth/login` with `LoginRequest` body → `LoginResponse`.
2. Derive WS URL: replace `http://` with `ws://` (or `https://` with `wss://`), append `/atp`.
3. Add `Authorization: Bearer <token>` header to the WS upgrade request via
   `tokio_tungstenite::connect_async_with_config` or by building a `http::Request`.
4. After upgrade, send `Subscribe { frame_type: "subscribe", events: vec!["*"] }` immediately.
5. Split the stream and return `AtpClient`.

#### `send`:

Serialise `cmd` to JSON string and send as `Message::Text`.

#### `next_frame`:

Read next `Message::Text`, deserialise to `Frame`. Skip `Message::Ping` (respond with
`Message::Pong`) and `Message::Close` (return `None`).

---

### 2. `atp/dispatcher.rs` — `Dispatcher`

The `Dispatcher` wraps an `AtpClient` and manages the request-reply correlation table.
It exposes a single high-level `call` method that:

1. Sends a `Cmd`.
2. Spawns a background reader task (or drives the stream in a select loop) to receive
   frames.
3. Stores in-flight `Cmd` ids in a `HashMap<String, oneshot::Sender<Reply>>`.
4. When a `Reply` arrives, route it to the matching `oneshot::Sender`.
5. Return `Reply` to the caller with a configurable `timeout`.

```rust
use std::collections::HashMap;
use tokio::sync::{oneshot, Mutex};
use std::sync::Arc;
use crate::atp::types::{Cmd, Reply, Frame};
use crate::error::ClientError;

pub struct Dispatcher {
    inner: Arc<Mutex<DispatcherInner>>,
}

struct DispatcherInner {
    // sink: WsSink,
    // pending: HashMap<String, oneshot::Sender<Reply>>,
    // …
}

impl Dispatcher {
    pub fn new(client: crate::atp::client::AtpClient) -> Self { … }

    /// Send `cmd` and wait for its `Reply` (by matching `id`), with a 30-second timeout.
    pub async fn call(&self, cmd: Cmd) -> Result<Reply, ClientError> { … }

    /// Subscribe to incoming events; returns a broadcast receiver.
    /// The background reader task forwards events to this channel.
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<crate::atp::types::Event> { … }
}
```

The background reader task runs in `tokio::spawn`. It reads frames from the WS stream
and either routes `Reply` frames to the pending table or broadcasts `Event` frames.

---

## Tests

Tests mock the WebSocket server using `tokio::net::TcpListener` + manual frame writing,
or use `tokio::sync::mpsc` channels to inject frames without a real network connection.
Prefer injecting frames through the read half of a `tokio::io::duplex` to avoid real
socket setup in unit tests.

```rust
#[cfg(test)]
mod tests {
    // Helper: build a Dispatcher backed by in-memory channels instead of
    // a real WS connection (unit-testable without a running server).
    // The test helper exposes a `Sender<Frame>` to inject inbound frames
    // and a `Receiver<String>` to capture outbound JSON.

    #[tokio::test]
    async fn call_returns_matching_reply() {
        // Arrange: inject dispatcher with fake transport
        // Act: dispatcher.call(cmd) while injecting Reply { id: cmd.id, ok: true }
        // Assert: returned Reply.ok == true
    }

    #[tokio::test]
    async fn call_returns_error_on_not_ok_reply() {
        // Inject Reply { ok: false, code: Some("EPERM"), message: … }
        // Assert: ClientError::Atp { code: "EPERM", … }
    }

    #[tokio::test]
    async fn event_broadcast_reaches_subscriber() {
        // Inject Event { kind: AgentOutput, … }
        // Assert: events() receiver gets the event
    }

    #[tokio::test]
    async fn call_times_out_if_no_reply() {
        // Do not inject any reply frame
        // Assert: returns ClientError::Timeout within reasonable duration
    }
}
```

---

## Implementation Order

1. Add `futures-util` and `http` to `avix-client-core/Cargo.toml` deps.
2. Implement `AtpClient::connect` and `send` / `next_frame`.
3. Implement `Dispatcher` with the background reader task.
4. Write and pass all four tests above.

---

## Success Criteria

- [ ] `AtpClient::connect` compiles (even if integration with a real server is tested manually)
- [ ] `Dispatcher::call` routes replies to the correct caller in unit tests
- [ ] `Dispatcher::events` broadcasts incoming events to all subscribers
- [ ] Timeout returns `ClientError::Timeout`
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
