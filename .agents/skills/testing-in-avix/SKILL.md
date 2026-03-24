---
name: testing-in-avix
description: Write, run, and verify tests in the Avix Rust workspace. Use when adding or modifying tests.
license: MIT
---

# Skill: Testing in the Avix Codebase

How to write, run, and verify tests in this Rust workspace. Read this before writing
any test code.

---

## Test locations

| Test type | Location | When to use |
|-----------|----------|-------------|
| Unit tests | Same file, `#[cfg(test)] mod tests { ... }` | Pure logic, single module |
| Integration tests | `crates/avix-core/tests/*.rs` | Cross-module flows, realistic scenarios |
| Benchmarks | `crates/avix-core/benches/` | Performance targets from CLAUDE.md |

`avix-client-core` uses in-file unit tests only — no `tests/` directory yet.

---

## Running tests

```bash
# Full workspace — always run this before committing
~/.cargo/bin/cargo test --workspace

# Single crate
~/.cargo/bin/cargo test -p avix-client-core
~/.cargo/bin/cargo test -p avix-core

# Single test by name (substring match)
~/.cargo/bin/cargo test -p avix-client-core notification::tests::add_increases

# Show stdout (useful for debugging panics)
~/.cargo/bin/cargo test -p avix-client-core -- --nocapture

# Include ignored tests
~/.cargo/bin/cargo test --workspace -- --include-ignored

# List all tests without running them
~/.cargo/bin/cargo test -p avix-client-core -- --list
```

---

## Async tests

Use `#[tokio::test]` for every async test. If the test is synchronous, use `#[test]`.
Mixing these up causes a compile error ("async keyword missing") or silently skips the
async runtime.

```rust
// Async test — requires tokio runtime
#[tokio::test]
async fn sends_event_to_subscriber() {
    let store = NotificationStore::new();
    store.add(Notification::from_sys_alert("info", "hello")).await;
    assert_eq!(store.unread_count().await, 1);
}

// Synchronous test — no async runtime needed
#[test]
fn backoff_caps_at_60_seconds() {
    let mut b = Duration::from_secs(1);
    for _ in 0..10 { b = b.saturating_mul(2).min(Duration::from_secs(60)); }
    assert_eq!(b, Duration::from_secs(60));
}
```

Always wrap async tests that could hang:

```rust
#[tokio::test]
async fn agent_exits_within_timeout() {
    use tokio::time::{timeout, Duration};
    timeout(Duration::from_secs(5), run_agent()).await
        .expect("timed out")
        .expect("agent error");
}
```

---

## File system tests

Always use `tempfile::tempdir()`. Never write to real paths or `$HOME`.

```rust
use tempfile::TempDir;

#[test]
fn save_and_reload_config() {
    let dir = TempDir::new().unwrap();          // cleaned up on drop
    let path = dir.path().join("config.json");
    save_json(&path, &my_config).unwrap();
    let loaded: MyConfig = load_json(&path).unwrap();
    assert_eq!(loaded.field, my_config.field);
}
```

`tempfile` is in `[dev-dependencies]` in `avix-client-core`. Add it to any crate that
needs it:

```toml
[dev-dependencies]
tempfile = "3"
```

---

## Testing broadcast channels and async state

Broadcast receivers can lag. In tests, use `try_recv()` for immediate assertion after
a send, and `recv().await` inside a `timeout` for event-driven assertions:

```rust
#[tokio::test]
async fn changed_signal_fires_on_add() {
    let store = NotificationStore::new();
    let mut rx = store.subscribe();

    store.add(Notification::from_sys_alert("info", "test")).await;

    // try_recv() works here because send happened before recv attempt
    assert!(rx.try_recv().is_ok());
}

#[tokio::test]
async fn event_arrives_within_1s() {
    use tokio::time::{timeout, Duration};
    let store = NotificationStore::new();
    let mut rx = store.subscribe();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        store.add(Notification::from_sys_alert("info", "x")).await;
    });
    timeout(Duration::from_secs(1), rx.recv()).await
        .expect("timed out")
        .expect("channel error");
}
```

---

## Testing with fake/mock infrastructure

When real infrastructure (WS server, running avix process) is unavailable, write an
inline fake in the test module. For the `Dispatcher`, the pattern is:

```rust
struct FakeDispatcher {
    replies: Arc<Mutex<HashMap<String, Reply>>>,
    captured: Arc<Mutex<Vec<Cmd>>>,
}

impl FakeDispatcher {
    fn set_reply(&self, domain_op: &str, reply: Reply) { ... }
    async fn call(&self, cmd: Cmd) -> Result<Reply, ClientError> {
        self.captured.try_lock().unwrap().push(cmd.clone());
        // look up and return the canned reply
    }
}
```

Tests that need a real WS connection should be marked `#[ignore]` with a clear reason:

```rust
#[tokio::test]
#[ignore = "requires a running avix gateway — run with --include-ignored in integration CI"]
async fn full_roundtrip_via_gateway() { ... }
```

Do **not** leave `todo!()` bodies in non-ignored tests — they panic and break CI.

---

## Atomic write test

Always verify that the `.tmp` file is cleaned up:

```rust
#[test]
fn atomic_write_leaves_no_tmp_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("data.json");
    save_json(&path, &vec!["hello"]).unwrap();
    assert!(!dir.path().join("data.json.tmp").exists());
    assert!(path.exists());
}
```

---

## Clippy and formatting — enforce in CI

After writing tests, always run:

```bash
~/.cargo/bin/cargo clippy --workspace -- -D warnings
~/.cargo/bin/cargo fmt --check
```

Common clippy lints that will fail CI:

| Lint | Fix |
|------|-----|
| `new_without_default` | Add `impl Default for T { fn default() -> Self { T::new() } }` |
| `needless_pass_by_value` | Change `fn f(x: String)` to `fn f(x: &str)` where appropriate |
| `redundant_closure` | Replace `\\|x\\| f(x)` with `f` |
| `match_wildcard_for_single_variants` | Replace `_ =>` with the explicit variant |
| `items_after_test_module` | Move `#[cfg(test)]` block to end of file |

---

## Test coverage target: 95%+

Check coverage with:

```bash
~/.cargo/bin/cargo tarpaulin --workspace --out Html --output-dir coverage/
```

Every public function in `avix-client-core` should have at least one test. Priority
order for coverage:
1. Error paths (non-ok replies, missing fields, connection refused)
2. State mutations (add, resolve, mark_read)
3. Serialisation roundtrips
4. Boundary conditions (MAX_LINES cap, backoff cap, empty arrays)

---

## Integration test file pattern

```rust
// crates/avix-core/tests/my_feature.rs

use avix_core::my_module::MyType;
use tempfile::TempDir;

#[tokio::test]
async fn full_flow() {
    let dir = TempDir::new().unwrap();
    // set up
    // act
    // assert
}
```

Add the file name to the top-level `tests/` directory — Cargo picks it up automatically
without any `mod` declaration.