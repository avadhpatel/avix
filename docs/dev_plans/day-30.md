# Day 30 — GUI SPA (Avix App)

> **Goal:** Implement the Avix App SPA desktop client: agent list with live status updates via ATP WebSocket, spawn/kill/pause/resume controls, live agent conversation feed, and `/proc/<pid>/status.yaml` VFS file viewer.

---

## Pre-flight: Verify Day 29

```bash
cargo test --workspace
cargo bench 2>&1 | grep -E "(atp_token|vfs_read|tool_registry|tool_name_mangle)" | head -10
# All benchmark outputs should show µs values within targets
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Confirm `avix-app` Crate Structure

```
crates/avix-app/src/
├── main.rs           ← entry point, boots Avix then launches GUI
├── ui/
│   ├── mod.rs
│   ├── app.rs        ← root App component
│   ├── agent_list.rs ← live agent list panel
│   ├── agent_detail.rs ← conversation feed + controls
│   ├── vfs_viewer.rs  ← /proc/<pid>/status.yaml viewer
│   └── llm_status.rs  ← provider health panel
└── atp_client/
    ├── mod.rs
    └── client.rs     ← ATP WebSocket client
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/gui_atp_client.rs`:

```rust
use avix_core::gateway::atp::{ATPCommand, ATPResponse};

// ── ATP client command serialisation ─────────────────────────────────────────

#[test]
fn spawn_command_serialises_correctly_for_atp() {
    let cmd = ATPCommand::AgentSpawn {
        req_id: "gui-001".into(),
        agent:  "researcher".into(),
        goal:   "Find Q3 data".into(),
        capabilities: vec!["web".into(), "fs".into()],
    };
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["cmd"], "agent.spawn");
    assert_eq!(v["id"], "gui-001");
    assert_eq!(v["params"]["agent"], "researcher");
}

#[test]
fn kill_command_serialises_correctly() {
    let cmd = ATPCommand::AgentKill { req_id: "gui-002".into(), pid: 57 };
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["cmd"], "agent.kill");
    assert_eq!(v["params"]["pid"], 57);
}

#[test]
fn pause_command_serialises() {
    let cmd = ATPCommand::AgentPause { req_id: "r".into(), pid: 57 };
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["cmd"], "agent.pause");
}

#[test]
fn resume_command_serialises() {
    let cmd = ATPCommand::AgentResume { req_id: "r".into(), pid: 57 };
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["cmd"], "agent.resume");
}

// ── Event parsing ─────────────────────────────────────────────────────────────

#[test]
fn parse_agent_status_changed_event() {
    use avix_core::gateway::atp::ATPEvent;
    let v = serde_json::json!({
        "event": "agent.status_changed",
        "data": {"pid": 57, "status": "paused"}
    });
    let event = ATPEvent::from_value(v).unwrap();
    assert!(matches!(event, ATPEvent::AgentStatusChanged { pid: 57, .. }));
}

#[test]
fn parse_tool_changed_event() {
    use avix_core::gateway::atp::ATPEvent;
    let v = serde_json::json!({
        "event": "tool.changed",
        "data": {"op": "removed", "tools": ["llm/complete"], "reason": "Provider down"}
    });
    let event = ATPEvent::from_value(v).unwrap();
    assert!(matches!(event, ATPEvent::ToolChanged { .. }));
}

#[test]
fn parse_hil_request_event() {
    use avix_core::gateway::atp::ATPEvent;
    let v = serde_json::json!({
        "event": "agent.hil_request",
        "data": {
            "pid": 57, "hilId": "hil-001",
            "scenario": "tool_call_approval",
            "toolName": "send_email",
            "reason": "Sending external email requires approval"
        }
    });
    let event = ATPEvent::from_value(v).unwrap();
    assert!(matches!(event, ATPEvent::HilRequest { .. }));
}

// ── ATP client response parsing ───────────────────────────────────────────────

#[test]
fn parse_ok_response() {
    let v = serde_json::json!({"id": "gui-001", "status": "ok", "result": {"pid": 57}});
    let resp = ATPResponse::from_value(v).unwrap();
    assert!(resp.is_ok());
    assert_eq!(resp.result().unwrap()["pid"], 57);
}

#[test]
fn parse_error_response() {
    let v = serde_json::json!({
        "id": "gui-001", "status": "error",
        "error": {"code": "EAUTH", "message": "Bad token"}
    });
    let resp = ATPResponse::from_value(v).unwrap();
    assert!(!resp.is_ok());
    assert_eq!(resp.error_code().unwrap(), "EAUTH");
}
```

---

## Step 3 — Implement GUI (Egui or similar)

The GUI crate itself uses your chosen framework (egui, tauri, or web). At minimum, the `atp_client` module must have correct serialisation and event handling. The visual rendering is framework-specific and does not require unit tests — integration/manual testing is sufficient for Day 30.

**`crates/avix-app/src/atp_client/client.rs`** key responsibilities:
- Connect to gateway via WebSocket
- Send ATP commands and correlate responses by `id`
- Dispatch incoming events to registered handlers
- Reconnect on disconnect with exponential backoff

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: all GUI ATP client serialisation tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check

# Build the app binary
cargo build -p avix-app
```

## Commit

```bash
git add -A
git commit -m "day-30: avix-app SPA — ATP client, command/event serialisation, GUI scaffold"
```

## Success Criteria

- [ ] All 4 agent control commands (spawn/kill/pause/resume) serialise correctly
- [ ] All 3 ATP event variants parse correctly
- [ ] OK and error response parsing work
- [ ] `cargo build -p avix-app` succeeds
- [ ] 15+ GUI ATP client tests pass
- [ ] 0 clippy warnings

---
---

