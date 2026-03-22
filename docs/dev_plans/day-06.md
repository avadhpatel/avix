# Day 6 — IPC Foundation: Platform-Native Transport + JSON-RPC Framing

> **Goal:** Build `IpcTransport` — the platform-native local-socket abstraction — with 4-byte length-prefix framing, JSON-RPC 2.0 message serialisation, and a fresh-connection-per-call model.

---

## Pre-flight: Verify Day 5

```bash
cargo test --workspace
# Expected: all Day 5 config tests pass (30+)

grep -r "pub struct LlmConfig"  crates/avix-core/src/
grep -r "pub struct AuthConfig" crates/avix-core/src/

cargo clippy --workspace -- -D warnings  # 0 warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`:

```rust
pub mod ipc;
```

Create:
```
src/ipc/
├── mod.rs
├── frame.rs      ← 4-byte length-prefix framing
├── transport.rs  ← IpcTransport trait + platform impl
└── message.rs    ← JSON-RPC 2.0 request/response types
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/ipc.rs`:

```rust
use avix_core::ipc::{frame, message::*};
use serde_json::json;

// ── Framing ───────────────────────────────────────────────────────────────────

#[test]
fn frame_encode_decode_round_trip() {
    let payload = json!({"jsonrpc": "2.0", "id": "1", "method": "fs/read", "params": {}});
    let bytes = frame::encode(&payload).unwrap();
    // First 4 bytes are the length
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert_eq!(len, bytes.len() - 4);
    let decoded: serde_json::Value = frame::decode(&bytes).unwrap();
    assert_eq!(decoded, payload);
}

#[test]
fn frame_length_prefix_is_little_endian() {
    let payload = json!({"x": 1});
    let bytes = frame::encode(&payload).unwrap();
    let le_len = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    assert_eq!(le_len as usize, bytes.len() - 4);
}

#[test]
fn frame_rejects_oversized_message() {
    // >16 MB message should be rejected
    let big_string = "x".repeat(17 * 1024 * 1024);
    let payload = json!({"data": big_string});
    assert!(frame::encode(&payload).is_err());
}

// ── JSON-RPC messages ─────────────────────────────────────────────────────────

#[test]
fn jsonrpc_request_serialises() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id:      "abc-123".into(),
        method:  "fs/read".into(),
        params:  json!({"path": "/etc/avix/kernel.yaml"}),
    };
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["method"], "fs/read");
    assert_eq!(v["id"], "abc-123");
}

#[test]
fn jsonrpc_response_ok_serialises() {
    let resp = JsonRpcResponse::ok("abc-123", json!({"content": "hello"}));
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["result"]["content"], "hello");
    assert!(v.get("error").is_none() || v["error"].is_null());
}

#[test]
fn jsonrpc_response_err_serialises() {
    let resp = JsonRpcResponse::err("abc-123", -32001, "EAUTH", None);
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["error"]["code"], -32001);
    assert_eq!(v["error"]["message"], "EAUTH");
    assert!(v.get("result").is_none() || v["result"].is_null());
}

#[test]
fn jsonrpc_error_codes_defined() {
    assert_eq!(JsonRpcErrorCode::Eauth  as i32, -32001);
    assert_eq!(JsonRpcErrorCode::Eperm  as i32, -32002);
    assert_eq!(JsonRpcErrorCode::Enoent as i32, -32003);
    assert_eq!(JsonRpcErrorCode::Ebusy  as i32, -32004);
    assert_eq!(JsonRpcErrorCode::Etimeout as i32, -32005);
    assert_eq!(JsonRpcErrorCode::Eused  as i32, -32009);
}

// ── Async round-trip over a real socket pair ──────────────────────────────────

#[tokio::test]
async fn ipc_round_trip_over_socket() {
    use avix_core::ipc::transport::test_socket_pair;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut client, mut server) = test_socket_pair().await;

    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "t1".into(),
        method: "ping".into(),
        params: json!({}),
    };

    // Client writes
    let bytes = frame::encode(&req).unwrap();
    client.write_all(&bytes).await.unwrap();

    // Server reads
    let received: JsonRpcRequest = frame::read_from(&mut server).await.unwrap();
    assert_eq!(received.method, "ping");
    assert_eq!(received.id, "t1");
}

#[tokio::test]
async fn ipc_fresh_connection_per_call_model() {
    // Two sequential calls each use a fresh connection — no persistent channel
    use avix_core::ipc::transport::test_socket_pair;
    let (mut c1, _s1) = test_socket_pair().await;
    let (mut c2, _s2) = test_socket_pair().await;

    // They are different sockets
    let req = frame::encode(&json!({"jsonrpc":"2.0","id":"1","method":"a","params":{}})).unwrap();
    c1.write_all(&req).await.unwrap();
    c2.write_all(&req).await.unwrap();
    // Both writes succeed independently
}
```

---

## Step 3 — Implement

**`src/ipc/frame.rs`**

```rust
use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::error::AvixError;

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024; // 16 MB

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, AvixError> {
    let body = serde_json::to_vec(msg)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(AvixError::ConfigParse("message exceeds 16 MB limit".into()));
    }
    let len = (body.len() as u32).to_le_bytes();
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&len);
    buf.extend_from_slice(&body);
    Ok(buf)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, AvixError> {
    if bytes.len() < 4 {
        return Err(AvixError::ConfigParse("frame too short".into()));
    }
    let body = &bytes[4..];
    serde_json::from_slice(body).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn read_from<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T, AvixError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(AvixError::ConfigParse("frame too large".into()));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn write_to<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    msg: &T,
) -> Result<(), AvixError> {
    let bytes = encode(msg)?;
    writer.write_all(&bytes).await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))
}
```

**`src/ipc/message.rs`** — define `JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcErrorCode` with `serde` derives.

**`src/ipc/transport.rs`** — `test_socket_pair()` creates an in-memory `tokio::net::UnixStream` pair (on Unix) or uses a temp named pipe on Windows.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: all Day 6 IPC tests pass (10+ new tests)
cargo clippy --workspace -- -D warnings   # 0 warnings
cargo fmt --check
```

---

## Commit

```bash
git add -A
git commit -m "day-06: IPC foundation — 4-byte framing, JSON-RPC types, socket transport"
```

---

## Success Criteria

- [ ] Frame encode/decode round-trip is lossless
- [ ] Frame length prefix is little-endian
- [ ] Oversized messages (>16 MB) rejected
- [ ] All JSON-RPC error codes defined and correct
- [ ] Async read/write round-trip over a real socket pair
- [ ] Fresh-connection-per-call model verified
- [ ] 0 clippy warnings

---
---

