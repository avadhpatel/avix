# Day 10 — Router: Concurrency Model + `_caller` Injection (Service-Level)

> **Goal:** Extend the router with per-call concurrency enforcement (`max_concurrent` from service unit files), proper `_caller` injection, and connection-level isolation. Each tool call opens a fresh connection; backpressure is applied when `max_concurrent` is reached.

---

## Pre-flight: Verify Day 9

```bash
cargo test --workspace     # all Day 9 auth tests pass (15+)
grep -r "pub struct AuthService" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Extend Router Module

Add to `src/router/mod.rs`:

```rust
pub mod concurrency;
pub use concurrency::ConcurrencyGuard;
```

---

## Step 2 — Write Tests First

Add to `crates/avix-core/tests/router.rs`:

```rust
use avix_core::router::concurrency::ConcurrencyLimiter;

// ── max_concurrent enforcement ────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_calls_within_limit_all_proceed() {
    let limiter = ConcurrencyLimiter::new(5);
    let mut guards = Vec::new();
    for _ in 0..5 {
        guards.push(limiter.acquire().await.unwrap());
    }
    assert_eq!(limiter.active_count().await, 5);
    // Dropping guards releases
    drop(guards);
    assert_eq!(limiter.active_count().await, 0);
}

#[tokio::test]
async fn acquire_beyond_limit_blocks_until_slot_available() {
    use std::time::Duration;
    let limiter = std::sync::Arc::new(ConcurrencyLimiter::new(2));
    let g1 = limiter.acquire().await.unwrap();
    let g2 = limiter.acquire().await.unwrap();

    let lim = std::sync::Arc::clone(&limiter);
    let handle = tokio::spawn(async move {
        lim.acquire().await.unwrap() // will block
    });

    // Release one slot
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(g1);

    // Third acquire should now succeed
    tokio::time::timeout(Duration::from_millis(200), handle).await
        .expect("should have acquired within timeout")
        .unwrap();
    drop(g2);
}

#[tokio::test]
async fn caller_scoped_limiter_tracks_per_caller() {
    use avix_core::types::Pid;
    use avix_core::router::concurrency::CallerScopedLimiter;

    let limiter = CallerScopedLimiter::new(2);
    let g1 = limiter.acquire(Pid::new(57)).await.unwrap();
    let g2 = limiter.acquire(Pid::new(57)).await.unwrap();

    // PID 57 is at limit; PID 58 should still get through
    let g3 = limiter.acquire(Pid::new(58)).await.unwrap();
    assert!(g3.is_valid());

    drop(g1);
    drop(g2);
    drop(g3);
}

// ── _caller always injected ───────────────────────────────────────────────────

#[test]
fn caller_injected_with_correct_pid_and_user() {
    use avix_core::router::inject_caller;
    use avix_core::types::Pid;
    use serde_json::json;
    let mut params = json!({});
    inject_caller(&mut params, Pid::new(57), "alice");
    assert_eq!(params["_caller"]["pid"], 57);
    assert_eq!(params["_caller"]["user"], "alice");
}

#[test]
fn caller_injection_preserves_existing_fields() {
    use avix_core::router::inject_caller;
    use avix_core::types::Pid;
    use serde_json::json;
    let mut params = json!({"path": "/test", "content": "hello"});
    inject_caller(&mut params, Pid::new(10), "bob");
    assert_eq!(params["path"], "/test");
    assert_eq!(params["content"], "hello");
}
```

---

## Step 3 — Implement

`ConcurrencyLimiter` wraps a `tokio::sync::Semaphore`. `CallerScopedLimiter` maintains a `HashMap<Pid, Arc<Semaphore>>` created lazily per caller.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-10: router concurrency limiter, caller-scoped limit, _caller injection"
```

## Success Criteria

- [ ] 12+ router + concurrency tests pass
- [ ] `max_concurrent` blocks excess calls and unblocks on slot release
- [ ] Caller-scoped limiter isolates per-PID limits
- [ ] `_caller` always injected with correct `pid` and `user`
- [ ] Existing params preserved after injection
- [ ] 0 clippy warnings
