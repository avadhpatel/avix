# Day 25 — Snapshot & Restore

> **Goal:** Implement full agent snapshot and restore: capture running agent state (messages, token, pending HIL) into a YAML blob at `/users/<username>/snapshots/<agent>-<timestamp>.yaml`, restore from snapshot to a new PID, and test round-trip fidelity.

---

## Pre-flight: Verify Day 24

```bash
cargo test --workspace
grep -r "CronScheduler"  crates/avix-core/src/
grep -r "MissedRunPolicy" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod snapshot;`

```
src/snapshot/
├── mod.rs
├── capture.rs    ← take snapshot of live executor state
├── restore.rs    ← restore executor from snapshot
└── store.rs      ← read/write snapshot YAML to MemFS
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/snapshot.rs`:

```rust
use avix_core::snapshot::{capture, restore, SnapshotStore};
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::types::{Pid, token::CapabilityToken};
use tempfile::tempdir;

fn token_with(tools: &[&str]) -> CapabilityToken {
    CapabilityToken { granted_tools: tools.iter().map(|s| s.to_string()).collect(), signature: "s".into() }
}

// ── Capture state ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_captures_agent_name_and_goal() {
    let registry = std::sync::Arc::new(avix_core::executor::MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(SpawnParams {
        pid: Pid::new(57), agent_name: "researcher".into(),
        goal: "Find Q3 revenue".into(), spawned_by: "alice".into(),
        token: token_with(&[]),
    }, registry).await.unwrap();

    let snap = capture(&executor).await.unwrap();
    assert_eq!(snap.agent_name, "researcher");
    assert_eq!(snap.goal, "Find Q3 revenue");
    assert_eq!(snap.spawned_by, "alice");
}

#[tokio::test]
async fn snapshot_captures_message_history() {
    let registry = std::sync::Arc::new(avix_core::executor::MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry(SpawnParams {
        pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
        spawned_by: "alice".into(), token: token_with(&[]),
    }, registry).await.unwrap();

    executor.push_message(serde_json::json!({"role": "user", "content": "hello"}));
    executor.push_message(serde_json::json!({"role": "assistant", "content": "world"}));

    let snap = capture(&executor).await.unwrap();
    assert_eq!(snap.messages.len(), 2);
    assert_eq!(snap.messages[0]["content"], "hello");
}

#[tokio::test]
async fn snapshot_captures_capability_token() {
    let registry = std::sync::Arc::new(avix_core::executor::MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(SpawnParams {
        pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
        spawned_by: "alice".into(),
        token: token_with(&["fs/read", "llm/complete"]),
    }, registry).await.unwrap();

    let snap = capture(&executor).await.unwrap();
    assert!(snap.token.granted_tools.contains(&"fs/read".to_string()));
    assert!(snap.token.granted_tools.contains(&"llm/complete".to_string()));
}

// ── Restore ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn restore_creates_executor_with_same_state() {
    let registry = std::sync::Arc::new(avix_core::executor::MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(SpawnParams {
        pid: Pid::new(57), agent_name: "researcher".into(),
        goal: "Find Q3 revenue".into(), spawned_by: "alice".into(),
        token: token_with(&["fs/read"]),
    }, registry.clone()).await.unwrap();

    let snap = capture(&executor).await.unwrap();
    let new_pid = Pid::new(99);
    let restored = restore(snap, new_pid, registry).await.unwrap();

    assert_eq!(restored.pid(), new_pid);
    assert_eq!(restored.agent_name(), "researcher");
    assert_eq!(restored.goal(), "Find Q3 revenue");
    assert!(restored.token().has_tool("fs/read"));
}

#[tokio::test]
async fn restore_preserves_message_history() {
    let registry = std::sync::Arc::new(avix_core::executor::MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry(SpawnParams {
        pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
        spawned_by: "alice".into(), token: token_with(&[]),
    }, registry.clone()).await.unwrap();

    executor.push_message(serde_json::json!({"role": "user", "content": "start"}));

    let snap = capture(&executor).await.unwrap();
    let restored = restore(snap, Pid::new(99), registry).await.unwrap();

    assert_eq!(restored.message_count(), 1);
}

// ── Persistence ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_save_and_load_from_vfs() {
    let tmp = tempdir().unwrap();
    let store = SnapshotStore::new_with_root(tmp.path());

    let snap = avix_core::snapshot::Snapshot {
        agent_name:  "researcher".into(),
        goal:        "Find data".into(),
        spawned_by:  "alice".into(),
        messages:    vec![],
        token:       token_with(&["fs/read"]),
        captured_at: chrono::Utc::now(),
        label:       Some("before-query".into()),
    };

    let path = store.save("alice", &snap).await.unwrap();
    assert!(path.contains("alice"));
    assert!(path.contains("researcher"));
    assert!(path.ends_with(".yaml"));

    let loaded = store.load(&path).await.unwrap();
    assert_eq!(loaded.agent_name, "researcher");
    assert_eq!(loaded.goal, "Find data");
    assert!(loaded.token.has_tool("fs/read"));
}

#[tokio::test]
async fn list_snapshots_for_agent() {
    let tmp = tempdir().unwrap();
    let store = SnapshotStore::new_with_root(tmp.path());

    let snap = avix_core::snapshot::Snapshot {
        agent_name: "researcher".into(), goal: "g".into(), spawned_by: "alice".into(),
        messages: vec![], token: token_with(&[]), captured_at: chrono::Utc::now(), label: None,
    };
    store.save("alice", &snap).await.unwrap();
    store.save("alice", &snap).await.unwrap();

    let list = store.list("alice", "researcher").await.unwrap();
    assert_eq!(list.len(), 2);
}
```

---

## Step 3 — Implement

`Snapshot` is a `serde`-serialisable struct. `capture(&executor)` clones state out. `restore(snap, new_pid, registry)` calls `spawn_with_registry` with restored params and then replays messages. `SnapshotStore` writes to `<root>/users/<username>/snapshots/<agent>-<timestamp>.yaml`.

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
git commit -m "day-25: snapshot/restore — capture, restore, VFS persistence, list"
```

## Success Criteria

- [ ] Snapshot captures agent name, goal, spawned_by
- [ ] Snapshot captures full message history
- [ ] Snapshot captures capability token with all granted tools
- [ ] Restore creates executor with new PID but same name/goal/token
- [ ] Restore preserves message history exactly
- [ ] Snapshot round-trips through YAML file
- [ ] `list` returns correct count for an agent
- [ ] 15+ tests pass, 0 clippy warnings
