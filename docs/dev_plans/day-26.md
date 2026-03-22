# Day 26 — Gateway Service (ATP over WebSocket)

> **Goal:** Implement `gateway.svc` — the external boundary between ATP clients (CLI, GUI, apps) and the internal IPC world. Accepts WebSocket connections, validates `ATPToken`, translates ATP commands into internal `kernel/` or `router/` IPC calls, and streams responses back.

---

## Pre-flight: Verify Day 25

```bash
cargo test --workspace
grep -r "pub fn capture"  crates/avix-core/src/
grep -r "pub fn restore"  crates/avix-core/src/
grep -r "SnapshotStore"   crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Add WebSocket Dependency

In `crates/avix-core/Cargo.toml`:

```toml
[dependencies]
tokio-tungstenite = "0.21"
```

Add to `src/lib.rs`: `pub mod gateway;`

```
src/gateway/
├── mod.rs
├── server.rs       ← GatewayServer, WebSocket accept loop
├── connection.rs   ← per-connection handler
├── atp/
│   ├── mod.rs
│   ├── command.rs  ← ATP command enum (agent.spawn, agent.kill, etc.)
│   └── response.rs ← ATP response/event types
└── translator.rs   ← ATP command → IPC call translation
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/gateway.rs`:

```rust
use avix_core::gateway::atp::{ATPCommand, ATPResponse};
use avix_core::gateway::translator::ATPTranslator;
use serde_json::json;

// ── ATP command parsing ───────────────────────────────────────────────────────

#[test]
fn parse_agent_spawn_command() {
    let msg = json!({
        "cmd": "agent.spawn",
        "id": "req-001",
        "params": {"agent": "researcher", "goal": "Find Q3", "capabilities": ["web"]}
    });
    let cmd = ATPCommand::from_value(msg).unwrap();
    assert!(matches!(cmd, ATPCommand::AgentSpawn { .. }));
    if let ATPCommand::AgentSpawn { req_id, agent, goal, .. } = cmd {
        assert_eq!(req_id, "req-001");
        assert_eq!(agent, "researcher");
        assert_eq!(goal, "Find Q3");
    }
}

#[test]
fn parse_agent_kill_command() {
    let msg = json!({"cmd": "agent.kill", "id": "req-002", "params": {"pid": 57}});
    let cmd = ATPCommand::from_value(msg).unwrap();
    assert!(matches!(cmd, ATPCommand::AgentKill { pid: 57, .. }));
}

#[test]
fn parse_fs_read_command() {
    let msg = json!({"cmd": "fs.read", "id": "r1", "params": {"path": "/users/alice/test.txt"}});
    let cmd = ATPCommand::from_value(msg).unwrap();
    assert!(matches!(cmd, ATPCommand::FsRead { .. }));
}

#[test]
fn parse_unknown_command_returns_error() {
    let msg = json!({"cmd": "nonexistent.cmd", "id": "r1", "params": {}});
    assert!(ATPCommand::from_value(msg).is_err());
}

// ── Response construction ─────────────────────────────────────────────────────

#[test]
fn atp_ok_response_format() {
    let resp = ATPResponse::ok("req-001", json!({"pid": 57}));
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["id"],           "req-001");
    assert_eq!(v["status"],       "ok");
    assert_eq!(v["result"]["pid"], 57);
    assert!(v.get("error").is_none() || v["error"].is_null());
}

#[test]
fn atp_err_response_format() {
    let resp = ATPResponse::err("req-001", "EAUTH", "Bad credentials");
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["status"], "error");
    assert_eq!(v["error"]["code"], "EAUTH");
}

// ── ATP command → IPC translation ────────────────────────────────────────────

#[test]
fn agent_spawn_translates_to_proc_spawn_ipc() {
    let translator = ATPTranslator::new();
    let cmd = ATPCommand::AgentSpawn {
        req_id: "r1".into(), agent: "researcher".into(),
        goal: "Find data".into(), capabilities: vec!["web".into()],
    };
    let ipc = translator.translate(&cmd).unwrap();
    assert_eq!(ipc.method, "kernel/proc/spawn");
    assert_eq!(ipc.params["agent"], "researcher");
}

#[test]
fn agent_kill_translates_to_proc_kill_ipc() {
    let translator = ATPTranslator::new();
    let cmd = ATPCommand::AgentKill { req_id: "r1".into(), pid: 57 };
    let ipc = translator.translate(&cmd).unwrap();
    assert_eq!(ipc.method, "kernel/proc/kill");
    assert_eq!(ipc.params["pid"], 57);
}

#[test]
fn fs_read_translates_to_fs_read_ipc() {
    let translator = ATPTranslator::new();
    let cmd = ATPCommand::FsRead {
        req_id: "r1".into(), path: "/users/alice/test.txt".into()
    };
    let ipc = translator.translate(&cmd).unwrap();
    assert_eq!(ipc.method, "kernel/fs/read");
    assert_eq!(ipc.params["path"], "/users/alice/test.txt");
}

// ── Token-gated access ────────────────────────────────────────────────────────

#[test]
fn translation_without_token_fails() {
    let translator = ATPTranslator::new();
    let cmd = ATPCommand::SysReboot { req_id: "r1".into() };
    // Translator without a session should reject sys-level commands
    let result = translator.translate_with_role(&cmd, avix_core::types::Role::Guest);
    assert!(result.is_err());
}

#[test]
fn admin_can_translate_sys_reboot() {
    let translator = ATPTranslator::new();
    let cmd = ATPCommand::SysReboot { req_id: "r1".into() };
    let result = translator.translate_with_role(&cmd, avix_core::types::Role::Admin);
    assert!(result.is_ok());
}

// ── GatewayServer integration ─────────────────────────────────────────────────

#[tokio::test]
async fn gateway_server_binds_and_accepts() {
    use avix_core::gateway::server::GatewayServer;

    let server = GatewayServer::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_port();
    assert!(port > 0);

    // Spawn server task
    let handle = tokio::spawn(async move {
        // Accept one connection then stop
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            server.accept_one()
        ).await;
    });

    // Connect a client
    use tokio_tungstenite::connect_async;
    let url = format!("ws://127.0.0.1:{}", port);
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        connect_async(url)
    ).await;
    // Connection should succeed (auth happens after WS handshake)
    assert!(result.is_ok());
    handle.await.ok();
}
```

---

## Step 3 — Implement

`ATPCommand` is an enum with variants for each ATP method. `ATPTranslator` maps commands to `IpcCall { method, params }`. `GatewayServer` runs a `tokio_tungstenite` accept loop; each connection is handled by `GatewayConnection` which reads ATP messages, validates the `ATPToken`, translates, and dispatches via IPC.

ATP never goes inside the system — `gateway.svc` is the sole boundary.

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
git commit -m "day-26: gateway.svc — ATP parsing, IPC translation, WebSocket accept"
```

## Success Criteria

- [ ] All ATP command variants parse correctly
- [ ] Unknown command returns parse error
- [ ] `ATPResponse.ok` and `ATPResponse.err` have correct JSON shape
- [ ] `agent.spawn` → `kernel/proc/spawn` IPC translation
- [ ] `agent.kill` → `kernel/proc/kill` IPC translation
- [ ] `sys.reboot` rejected for guest/user role, allowed for admin
- [ ] WebSocket server binds and accepts connection
- [ ] 20+ tests pass, 0 clippy warnings

---
---

