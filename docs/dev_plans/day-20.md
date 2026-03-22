# Day 20 — Service Lifecycle + ServiceToken

> **Goal:** Implement the service startup contract: kernel issues a `ServiceToken` at spawn, service calls `ipc.register` with the token, kernel validates and registers the service's socket and tools. Also implement dynamic `ipc.tool-add` and `ipc.tool-remove`.

---

## Pre-flight: Verify Day 19

```bash
cargo test --workspace
grep -r "pub struct ToolRegistry" crates/avix-core/src/
grep -r "ToolVisibility"          crates/avix-core/src/
grep -r "drain"                   crates/avix-core/src/tool_registry/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod service;`

```
src/service/
├── mod.rs
├── token.rs       ← ServiceToken
├── lifecycle.rs   ← spawn + register + shutdown
└── ipc_calls.rs   ← ipc.register, ipc.tool-add, ipc.tool-remove handlers
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/service_lifecycle.rs`:

```rust
use avix_core::service::{ServiceManager, ServiceSpawnRequest, IpcRegisterRequest};

// ── ServiceToken issuance ─────────────────────────────────────────────────────

#[tokio::test]
async fn service_token_issued_at_spawn() {
    let mgr = ServiceManager::new_for_test();
    let token = mgr.spawn_and_get_token(ServiceSpawnRequest {
        name:   "test-svc".into(),
        binary: "/services/test-svc/bin/test-svc".into(),
    }).await.unwrap();

    assert!(!token.token_str.is_empty());
    assert_eq!(token.service_name, "test-svc");
}

// ── ipc.register ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn ipc_register_succeeds_with_valid_token() {
    let mgr = ServiceManager::new_for_test();
    let token = mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "test-svc".into(), binary: "binary".into(),
    }).await.unwrap();

    let result = mgr.handle_ipc_register(IpcRegisterRequest {
        token:    token.token_str.clone(),
        name:     "test-svc".into(),
        endpoint: "/run/avix/services/test-svc.sock".into(),
        tools:    vec!["test-svc/hello".into()],
    }).await.unwrap();

    assert!(result.registered);
    assert!(result.pid.as_u32() > 0);
}

#[tokio::test]
async fn ipc_register_fails_with_invalid_token() {
    let mgr = ServiceManager::new_for_test();
    let result = mgr.handle_ipc_register(IpcRegisterRequest {
        token:    "bad-token".into(),
        name:     "evil-svc".into(),
        endpoint: "/run/avix/services/evil.sock".into(),
        tools:    vec![],
    }).await;
    assert!(result.is_err());
}

// ── Service env vars ──────────────────────────────────────────────────────────

#[tokio::test]
async fn service_env_contains_required_vars() {
    let mgr = ServiceManager::new_for_test();
    mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "github-svc".into(), binary: "binary".into(),
    }).await.unwrap();

    let env = mgr.service_env("github-svc").await.unwrap();
    assert!(env.contains_key("AVIX_KERNEL_SOCK"));
    assert!(env.contains_key("AVIX_ROUTER_SOCK"));
    assert!(env.contains_key("AVIX_SVC_SOCK"));
    assert!(env.contains_key("AVIX_SVC_TOKEN"));
}

#[tokio::test]
#[cfg(unix)]
async fn service_env_sockets_end_with_sock_on_unix() {
    let mgr = ServiceManager::new_for_test();
    mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "github-svc".into(), binary: "binary".into(),
    }).await.unwrap();

    let env = mgr.service_env("github-svc").await.unwrap();
    assert!(env["AVIX_KERNEL_SOCK"].ends_with(".sock"));
    assert!(env["AVIX_ROUTER_SOCK"].ends_with(".sock"));
}

// ── Dynamic tool-add / tool-remove ────────────────────────────────────────────

#[tokio::test]
async fn ipc_tool_add_registers_tools() {
    let (mgr, tool_reg) = ServiceManager::new_with_registry();
    let token = mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "github-svc".into(), binary: "binary".into(),
    }).await.unwrap();

    mgr.handle_tool_add(token.token_str.clone(), vec!["github/list-prs".into()]).await.unwrap();

    assert!(tool_reg.lookup("github/list-prs").await.is_ok());
}

#[tokio::test]
async fn ipc_tool_remove_deregisters_tools() {
    let (mgr, tool_reg) = ServiceManager::new_with_registry();
    let token = mgr.spawn_and_get_token(ServiceSpawnRequest {
        name: "github-svc".into(), binary: "binary".into(),
    }).await.unwrap();

    mgr.handle_tool_add(token.token_str.clone(), vec!["github/list-prs".into()]).await.unwrap();
    mgr.handle_tool_remove(token.token_str.clone(), vec!["github/list-prs".into()], "down", false).await.unwrap();

    assert!(tool_reg.lookup("github/list-prs").await.is_err());
}

#[tokio::test]
async fn ipc_tool_add_with_invalid_token_fails() {
    let (mgr, _) = ServiceManager::new_with_registry();
    let result = mgr.handle_tool_add("bad-token".into(), vec!["svc/tool".into()]).await;
    assert!(result.is_err());
}
```

---

## Step 3 — Implement

`ServiceToken` is a UUID-based token associated with a service name. `ServiceManager` stores `HashMap<service_name, ServiceToken>` and `HashMap<token_str, service_name>`. `handle_ipc_register` validates token, registers service socket with the router. `handle_tool_add/remove` validates token and delegates to `ToolRegistry`.

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
git commit -m "day-20: service lifecycle — ServiceToken, ipc.register, tool-add/remove"
```

## Success Criteria

- [ ] `ServiceToken` issued at spawn; token is non-empty
- [ ] `ipc.register` succeeds with valid token, fails with invalid
- [ ] Service env vars contain all four required socket vars
- [ ] Socket paths end with `.sock` on Unix
- [ ] `ipc.tool-add` registers tools in registry
- [ ] `ipc.tool-remove` deregisters tools
- [ ] `ipc.tool-add` with invalid token is rejected
- [ ] 20+ tests pass, 0 clippy warnings
