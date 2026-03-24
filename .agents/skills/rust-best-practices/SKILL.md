---
name: rust-best-practices
description: Apply Rust best practices tailored for the Avix codebase. Use when writing, refactoring, or reviewing Rust code.
license: MIT
---

# Skill: Rust Best Practices for Avix

Reference guide for writing idiomatic, safe Rust in this codebase. Apply these rules
in every file you touch.

---

## Error handling

### Library code (`avix-core`, `avix-client-core`)

Use `thiserror` — define typed errors, never use `Box<dyn Error>` or `anyhow` directly.

```rust
// Good
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("ATP error {code}: {message}")]
    Atp { code: String, message: String },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// Bad
pub fn connect() -> Result<(), Box<dyn std::error::Error>> { ... }
```

Wrap unexpected/external errors with `ClientError::Other(anyhow::anyhow!("context: {e}"))`.

### Application code (`avix-cli`, `avix-app`)

Use `anyhow::Result<T>` at the top level. Add context with `.with_context(|| "...")`.

```rust
// Good
let cfg = ClientConfig::load().with_context(|| "loading client config")?;

// Bad
let cfg = ClientConfig::load().unwrap();
```

### Never `.unwrap()` in non-test code

```rust
// Good — propagate
let val = some_result?;

// Good — explicit handling
let val = some_result.unwrap_or_else(|_| default_value);

// Bad
let val = some_result.unwrap();
```

In tests, `.unwrap()` is fine — it produces a clear panic with a backtrace.

---

## Async patterns

### Shared mutable state

Use `Arc<tokio::sync::RwLock<T>>` for state read often and written rarely.
Use `Arc<tokio::sync::Mutex<T>>` for state written frequently or held briefly.

```rust
// Good — short-lived lock, always drop before await
let val = {
    let guard = state.read().await;
    guard.something.clone()
};
// Do async work here without holding the lock
do_async_thing(val).await;

// Bad — holding a lock across an await point
let guard = state.read().await;
do_async_thing(guard.something.clone()).await;  // lock held across await!
```

### Background tasks

Always store the `JoinHandle` — dropping it detaches the task silently.

```rust
// Good
struct Foo {
    _handle: tokio::task::JoinHandle<()>,
}

// Bad — handle dropped immediately, task silently detached
tokio::spawn(async move { ... });
```

### Channels

- `tokio::sync::broadcast` — fan-out, multiple readers. Capacity 256+ for event buses.
  Use `resubscribe()` to hand out new receivers; never clone a `Receiver`.
- `tokio::sync::mpsc` — single-producer or multi-producer, single consumer pipelines.
- `tokio::sync::oneshot` — single reply to a single caller (request/response).
- Prefer `try_send` / `try_recv` in hot paths; log on `Lagged` not panic.

### Timeouts

Wrap every async operation that could hang:

```rust
use tokio::time::{timeout, Duration};

let result = timeout(Duration::from_secs(30), some_future)
    .await
    .map_err(|_| ClientError::Timeout)?;
```

---

## Serde patterns

### Internally-tagged enums

When an enum uses `#[serde(tag = "type")]`, the tag field is consumed and **not** passed
to the inner struct. Any inner struct field also named `type` must skip deserialisation:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Frame {
    Reply(Reply),
    Event(Event),
}

#[derive(Serialize, Deserialize)]
pub struct Reply {
    // skip_deserializing because Frame's tag consumed "type" already
    #[serde(rename = "type", skip_deserializing, default)]
    pub frame_type: String,
    pub id: String,
    pub ok: bool,
}
```

### Unsized generics

`save_json<T: Serialize>` cannot accept `&[T]` (a slice is `!Sized`). Add `?Sized`:

```rust
pub fn save_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), ClientError>
```

### Atomic writes

Never write directly to the final path — use a `.tmp` file then rename:

```rust
let tmp = path.with_extension("tmp");
fs::write(&tmp, &json)?;
fs::rename(&tmp, path)?;  // atomic on POSIX
```

---

## Struct design

### Always implement `Debug`

All `pub` structs must implement `Debug`. Use `#[derive(Debug)]` when all fields
implement it. Add a manual impl when fields don't (e.g. `JoinHandle`, broadcast channels):

```rust
impl std::fmt::Debug for EventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish_non_exhaustive()  // hides non-Debug fields
    }
}
```

### `Default` for types with `new()`

If a type has a `new()` constructor with no required arguments, add a `Default` impl:

```rust
impl Default for NotificationStore {
    fn default() -> Self { Self::new() }
}
```

This satisfies clippy's `clippy::new_without_default` lint.

---

## Closures and lifetimes

### Closures that spawn tasks must move ownership

```rust
// Bad — closure borrows, but tokio::spawn requires 'static
let count = Arc::new(AtomicUsize::new(0));
let f = || { count.fetch_add(1, Ordering::SeqCst); ... };  // borrows count

// Good — clone before moving
let count = Arc::new(AtomicUsize::new(0));
let count_c = Arc::clone(&count);
let f = move || { count_c.fetch_add(1, Ordering::SeqCst); ... };
```

### Async closures returning futures

When a closure returns a `Future`, the returned future must be `Send + 'static` to work
with `tokio::spawn`. Use `async move { ... }` inside the closure:

```rust
let connect_fn = move || {
    let url = url.clone();
    async move { AtpClient::connect(&url, ...).await }
};
```

---

## Tracing (logging)

```rust
use tracing::{debug, info, warn, error};

// Lifecycle events
info!("Agent spawned — pid={}", pid);

// Per-turn / hot-path events
debug!("Tool call dispatched — tool={} cmd_id={}", tool, id);

// Recoverable issues
warn!("Reconnect attempt {attempt}/5 failed — backoff={backoff:?}");

// Non-recoverable / unexpected
error!("Dispatcher reader error: {:?}", e);
```

Never use `println!` in library code (`avix-core`, `avix-client-core`). It is acceptable
only in the binary entry points (`avix-cli`, `avix-app`) for user-facing output.

---

## Imports

Group imports in this order, separated by blank lines:

```rust
// 1. std
use std::collections::HashMap;
use std::sync::Arc;

// 2. external crates
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

// 3. crate-internal
use crate::atp::types::{Cmd, Reply};
use crate::error::ClientError;
```

Run `cargo fmt` to normalise — it reorders within groups automatically.

---

## Module declaration

A file must be declared in its parent `mod.rs` or `lib.rs` before it compiles:

```rust
// atp/mod.rs
pub mod client;
pub mod dispatcher;
pub mod event_emitter;   // ← must be here or the file is silently ignored
pub mod types;
```

Forgetting this is a common mistake when adding new files — tests in the new file will
never run and the compiler will not warn about the orphaned file.