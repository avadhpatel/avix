# Day 21 — Kernel Syscalls: 32 Calls Across 6 Domains

> **Goal:** Implement all 32 kernel syscalls across the six domains: `proc/` (agent lifecycle), `fs/` (VFS), `cap/` (capability), `sys/` (system), `sched/` (scheduling), `snap/` (snapshot). Each syscall validates the caller's `CapabilityToken` and returns a typed result.

---

## Pre-flight: Verify Day 20

```bash
cargo test --workspace
grep -r "ServiceManager"    crates/avix-core/src/
grep -r "handle_tool_add"   crates/avix-core/src/
grep -r "handle_ipc_register" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod syscall;`

```
src/syscall/
├── mod.rs
├── handler.rs       ← SyscallHandler dispatch table
├── domain/
│   ├── proc_.rs     ← kernel/proc/spawn, kill, list, info, wait, signal
│   ├── fs_.rs       ← kernel/fs/read, write, list, exists, delete, watch
│   ├── cap_.rs      ← kernel/cap/issue, validate, revoke, policy
│   ├── sys_.rs      ← kernel/sys/info, boot-log, reboot
│   ├── sched_.rs    ← kernel/sched/cron-add, cron-remove, cron-list
│   └── snap_.rs     ← kernel/snap/save, restore, list, delete
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/syscalls.rs`:

```rust
use avix_core::syscall::SyscallHandler;
use avix_core::types::{Pid, token::CapabilityToken, Role};
use serde_json::json;

fn admin_token() -> CapabilityToken {
    CapabilityToken { granted_tools: vec!["*".into()], signature: "admin".into() }
}

fn user_token(tools: &[&str]) -> CapabilityToken {
    CapabilityToken {
        granted_tools: tools.iter().map(|s| s.to_string()).collect(),
        signature: "user".into(),
    }
}

// ── proc domain ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn proc_spawn_creates_process() {
    let handler = SyscallHandler::new_for_test();
    let result = handler.call(
        "kernel/proc/spawn",
        json!({"agent": "researcher", "goal": "Find data", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice",
    ).await.unwrap();
    let pid = result["pid"].as_u64().unwrap();
    assert!(pid > 0);
}

#[tokio::test]
async fn proc_spawn_returns_pid_that_exists_in_process_table() {
    let handler = SyscallHandler::new_for_test();
    let result = handler.call(
        "kernel/proc/spawn",
        json!({"agent": "researcher", "goal": "g", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice",
    ).await.unwrap();
    let pid = Pid::new(result["pid"].as_u64().unwrap() as u32);
    let status = handler.call(
        "kernel/proc/info",
        json!({"pid": pid.as_u32()}),
        &admin_token(), Pid::new(0), "alice",
    ).await.unwrap();
    assert_eq!(status["name"], "researcher");
}

#[tokio::test]
async fn proc_kill_removes_from_table() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/proc/spawn",
        json!({"agent": "a", "goal": "g", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let pid = r["pid"].as_u64().unwrap() as u32;

    handler.call("kernel/proc/kill",
        json!({"pid": pid}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let info = handler.call("kernel/proc/info",
        json!({"pid": pid}),
        &admin_token(), Pid::new(0), "alice").await;
    assert!(info.is_err()); // not found after kill
}

#[tokio::test]
async fn proc_list_returns_spawned_processes() {
    let handler = SyscallHandler::new_for_test();
    for name in &["r1", "r2", "r3"] {
        handler.call("kernel/proc/spawn",
            json!({"agent": name, "goal": "g", "capabilities": []}),
            &admin_token(), Pid::new(0), "alice").await.unwrap();
    }
    let r = handler.call("kernel/proc/list", json!({}), &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(r["processes"].as_array().unwrap().len() >= 3);
}

#[tokio::test]
async fn proc_signal_delivers_to_target() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/proc/spawn",
        json!({"agent": "a", "goal": "g", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let pid = r["pid"].as_u64().unwrap() as u32;

    let result = handler.call("kernel/proc/signal",
        json!({"pid": pid, "signal": "SIGPAUSE", "payload": {}}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert_eq!(result["status"], "delivered");
}

// ── fs domain ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fs_write_and_read_round_trip() {
    let handler = SyscallHandler::new_for_test();
    handler.call("kernel/fs/write",
        json!({"path": "/users/alice/workspace/test.txt", "content": "hello"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let r = handler.call("kernel/fs/read",
        json!({"path": "/users/alice/workspace/test.txt"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    assert_eq!(r["content"].as_str().unwrap(), "hello");
}

#[tokio::test]
async fn fs_exists_returns_bool() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/fs/exists",
        json!({"path": "/etc/avix/kernel.yaml"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(r["exists"].is_boolean());
}

// ── cap domain ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cap_issue_returns_signed_token() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/cap/issue",
        json!({"agent_pid": 57, "tools": ["fs/read", "llm/complete"]}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(!r["token"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn cap_validate_returns_ok_for_valid_token() {
    let handler = SyscallHandler::new_for_test();
    let issued = handler.call("kernel/cap/issue",
        json!({"agent_pid": 57, "tools": ["fs/read"]}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let token_str = issued["token"].as_str().unwrap().to_string();

    let valid = handler.call("kernel/cap/validate",
        json!({"token": token_str, "tool": "fs/read"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert_eq!(valid["valid"], true);
}

#[tokio::test]
async fn cap_revoke_makes_validation_fail() {
    let handler = SyscallHandler::new_for_test();
    let issued = handler.call("kernel/cap/issue",
        json!({"agent_pid": 57, "tools": ["fs/read"]}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let token_str = issued["token"].as_str().unwrap().to_string();

    handler.call("kernel/cap/revoke",
        json!({"token": token_str}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let valid = handler.call("kernel/cap/validate",
        json!({"token": token_str, "tool": "fs/read"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert_eq!(valid["valid"], false);
}

// ── sys domain ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sys_info_returns_version_and_uptime() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/sys/info", json!({}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(r["version"].is_string());
    assert!(r["uptimeMs"].is_number());
}

#[tokio::test]
async fn sys_boot_log_returns_phase_entries() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/sys/boot-log", json!({}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(r["entries"].as_array().is_some());
}

// ── sched domain ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn sched_cron_add_and_list() {
    let handler = SyscallHandler::new_for_test();
    handler.call("kernel/sched/cron-add",
        json!({"name": "daily-report", "schedule": "0 0 * * *", "agent": "reporter", "goal": "generate report"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let r = handler.call("kernel/sched/cron-list", json!({}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let jobs = r["jobs"].as_array().unwrap();
    assert!(jobs.iter().any(|j| j["name"] == "daily-report"));
}

#[tokio::test]
async fn sched_cron_remove() {
    let handler = SyscallHandler::new_for_test();
    handler.call("kernel/sched/cron-add",
        json!({"name": "temp-job", "schedule": "* * * * *", "agent": "a", "goal": "g"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    handler.call("kernel/sched/cron-remove", json!({"name": "temp-job"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let r = handler.call("kernel/sched/cron-list", json!({}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(!r["jobs"].as_array().unwrap().iter().any(|j| j["name"] == "temp-job"));
}

// ── snap domain ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn snap_save_and_list() {
    let handler = SyscallHandler::new_for_test();
    let r = handler.call("kernel/proc/spawn",
        json!({"agent": "researcher", "goal": "research", "capabilities": []}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    let pid = r["pid"].as_u64().unwrap() as u32;

    handler.call("kernel/snap/save",
        json!({"pid": pid, "label": "before-search"}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();

    let list = handler.call("kernel/snap/list",
        json!({"pid": pid}),
        &admin_token(), Pid::new(0), "alice").await.unwrap();
    assert!(list["snapshots"].as_array().unwrap().iter()
        .any(|s| s["label"] == "before-search"));
}

// ── Unauthorised syscall is rejected ─────────────────────────────────────────

#[tokio::test]
async fn sys_reboot_requires_admin() {
    let handler = SyscallHandler::new_for_test();
    let low_token = user_token(&["fs/read"]);
    let result = handler.call("kernel/sys/reboot", json!({}), &low_token, Pid::new(57), "bob").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("perm") ||
            result.unwrap_err().to_string().contains("EPERM"));
}
```

---

## Step 3 — Implement

`SyscallHandler` holds references to `ProcessTable`, `MemFs`, `SignalBus`, `ToolRegistry`, `SessionStore`. Each `kernel/<domain>/<verb>` method extracts params, validates caller permission, dispatches to the appropriate subsystem, and returns `Value`.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 25+ syscall tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-21: 32 kernel syscalls — proc, fs, cap, sys, sched, snap domains"
```

## Success Criteria

- [ ] `proc/spawn` → PID visible in `proc/info` and `proc/list`
- [ ] `proc/kill` → process no longer found
- [ ] `proc/signal` delivers signal to target
- [ ] `fs/write` + `fs/read` round-trips content
- [ ] `cap/issue` → `cap/validate` → `cap/revoke` lifecycle
- [ ] `sys/info` returns `version` and `uptimeMs`
- [ ] `sched/cron-add` → visible in `cron-list` → `cron-remove` removes it
- [ ] `snap/save` → visible in `snap/list`
- [ ] `sys/reboot` rejected without admin token (EPERM)
- [ ] 25+ tests pass, 0 clippy warnings

---
---

