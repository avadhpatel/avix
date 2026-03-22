# Day 7 — Router Service

> **Goal:** Build `router.svc` — the internal IPC backbone that receives all tool call requests and forwards them to the correct service socket. Implements the fresh-connection-per-call routing model, service registry, and the `_caller` injection.

---

## Pre-flight: Verify Day 6

```bash
cargo test --workspace     # all Day 6 IPC tests pass
grep -r "pub fn encode"    crates/avix-core/src/ipc/
grep -r "JsonRpcRequest"   crates/avix-core/src/ipc/
cargo clippy --workspace -- -D warnings   # 0 warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod router;`

```
src/router/
├── mod.rs
├── registry.rs   ← service endpoint registry
└── router.rs     ← routing logic
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/router.rs`:

```rust
use avix_core::router::{ServiceRegistry, Router};

// ── Registry ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn register_and_lookup_service() {
    let reg = ServiceRegistry::new();
    reg.register("github-svc", "/run/avix/services/github-svc.sock").await;
    let ep = reg.lookup("github-svc").await.unwrap();
    assert_eq!(ep, "/run/avix/services/github-svc.sock");
}

#[tokio::test]
async fn lookup_unregistered_returns_none() {
    let reg = ServiceRegistry::new();
    assert!(reg.lookup("ghost-svc").await.is_none());
}

#[tokio::test]
async fn deregister_removes_service() {
    let reg = ServiceRegistry::new();
    reg.register("svc", "/run/avix/services/svc.sock").await;
    reg.deregister("svc").await;
    assert!(reg.lookup("svc").await.is_none());
}

// ── Tool-to-service routing ───────────────────────────────────────────────────

#[tokio::test]
async fn route_tool_to_correct_service() {
    let reg = ServiceRegistry::new();
    reg.register_tool("fs/read",  "memfs-svc").await;
    reg.register_tool("fs/write", "memfs-svc").await;
    reg.register_tool("llm/complete", "llm-svc").await;

    assert_eq!(reg.service_for_tool("fs/read").await.unwrap(), "memfs-svc");
    assert_eq!(reg.service_for_tool("llm/complete").await.unwrap(), "llm-svc");
}

#[tokio::test]
async fn route_unknown_tool_returns_none() {
    let reg = ServiceRegistry::new();
    assert!(reg.service_for_tool("ghost/tool").await.is_none());
}

// ── _caller injection ─────────────────────────────────────────────────────────

#[test]
fn caller_injected_into_params() {
    use avix_core::router::inject_caller;
    use avix_core::types::Pid;
    use serde_json::json;

    let mut params = json!({"path": "/etc/test.yaml"});
    inject_caller(&mut params, Pid::new(57), "alice");
    assert_eq!(params["_caller"]["pid"], 57);
    assert_eq!(params["_caller"]["user"], "alice");
}

#[test]
fn caller_does_not_overwrite_existing_params() {
    use avix_core::router::inject_caller;
    use avix_core::types::Pid;
    use serde_json::json;

    let mut params = json!({"path": "/test"});
    inject_caller(&mut params, Pid::new(57), "alice");
    // Original params preserved
    assert_eq!(params["path"], "/test");
}

// ── Concurrent routing ────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_tool_registrations() {
    use std::sync::Arc;
    let reg = Arc::new(ServiceRegistry::new());
    let mut handles = Vec::new();

    for i in 0..50u32 {
        let r = Arc::clone(&reg);
        handles.push(tokio::spawn(async move {
            r.register_tool(&format!("svc/tool-{i}"), "test-svc").await;
        }));
    }
    for h in handles { h.await.unwrap(); }

    assert_eq!(reg.tool_count().await, 50);
}
```

---

## Step 3 — Implement

**`src/router/registry.rs`** — `Arc<RwLock<HashMap>>` for services and tools. `register`, `deregister`, `lookup`, `register_tool`, `service_for_tool`, `tool_count`.

**`src/router/mod.rs`** — re-exports + `inject_caller(params: &mut Value, pid: Pid, user: &str)` free function.

---

## Step 4 — Verify

```bash
cargo test --workspace     # all Day 7 router tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Commit

```bash
git add -A
git commit -m "day-07: router service registry, tool routing, _caller injection"
```

## Success Criteria

- [ ] Service register/lookup/deregister all work
- [ ] Tool-to-service routing resolves correctly
- [ ] `_caller` injected with `pid` and `user` into every request params
- [ ] Concurrent registrations (50) all visible after join
- [ ] 12+ tests pass, 0 clippy warnings

---
---

