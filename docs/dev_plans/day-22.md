# Day 22 — Pipes: IPC Channels Between Agents

> **Goal:** Implement the full pipe lifecycle: `pipe/open`, `pipe/write`, `pipe/read`, `pipe/close`, `pipe/status`. Pipes are represented as `/proc/<pid>/pipes/<id>.yaml`. Implement backpressure (capacity limit) and SIGPIPE on write to closed pipe.

---

## Pre-flight: Verify Day 21

```bash
cargo test --workspace
grep -r "SyscallHandler"   crates/avix-core/src/
grep -r "kernel/proc/spawn" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod pipe;`

```
src/pipe/
├── mod.rs
├── registry.rs   ← PipeRegistry
└── pipe.rs       ← Pipe struct, lifecycle
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/pipe.rs`:

```rust
use avix_core::pipe::{PipeRegistry, PipeStatus};
use avix_core::types::Pid;
use serde_json::json;

// ── Open ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pipe_open_returns_id() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    assert!(!id.is_empty());
}

#[tokio::test]
async fn pipe_status_after_open_is_open() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    let status = reg.status(&id).await.unwrap();
    assert_eq!(status, PipeStatus::Open);
}

// ── Write/Read ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn write_and_read_message() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();

    reg.write(&id, json!({"event": "data", "value": 42})).await.unwrap();
    let msg = reg.read(&id).await.unwrap();
    assert_eq!(msg["value"], 42);
}

#[tokio::test]
async fn read_empty_pipe_returns_none() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    assert!(reg.try_read(&id).await.unwrap().is_none());
}

#[tokio::test]
async fn multiple_messages_ordered() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();

    for i in 0..5u32 {
        reg.write(&id, json!({"seq": i})).await.unwrap();
    }

    for i in 0..5u32 {
        let msg = reg.read(&id).await.unwrap();
        assert_eq!(msg["seq"], i);
    }
}

// ── Backpressure ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn write_blocks_when_capacity_full() {
    use std::time::Duration;
    let reg = std::sync::Arc::new(PipeRegistry::new_with_capacity(2));
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();

    reg.write(&id, json!({})).await.unwrap();
    reg.write(&id, json!({})).await.unwrap();

    // Third write should block until a reader consumes
    let r = std::sync::Arc::clone(&reg);
    let i = id.clone();
    let write_handle = tokio::spawn(async move {
        r.write(&i, json!({"third": true})).await.unwrap();
    });

    // Reader consumes one
    tokio::time::sleep(Duration::from_millis(20)).await;
    let _ = reg.read(&id).await.unwrap();

    tokio::time::timeout(Duration::from_millis(200), write_handle)
        .await.expect("write should unblock").unwrap();
}

// ── Close ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn close_sets_status_closed() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    reg.close(&id).await.unwrap();
    assert_eq!(reg.status(&id).await.unwrap(), PipeStatus::Closed);
}

#[tokio::test]
async fn write_to_closed_pipe_returns_sigpipe_error() {
    let reg = PipeRegistry::new();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    reg.close(&id).await.unwrap();

    let result = reg.write(&id, json!({"data": "hello"})).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("SIGPIPE") ||
            result.unwrap_err().to_string().contains("closed"));
}

// ── VFS representation ────────────────────────────────────────────────────────

#[tokio::test]
async fn pipe_appears_in_vfs_after_open() {
    let (reg, memfs) = PipeRegistry::new_with_vfs();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    let path = format!("/proc/57/pipes/{}.yaml", id);
    let vfs_path = avix_core::memfs::VfsPath::parse(&path).unwrap();
    assert!(memfs.exists(&vfs_path).await);
}

#[tokio::test]
async fn pipe_removed_from_vfs_after_close() {
    let (reg, memfs) = PipeRegistry::new_with_vfs();
    let id = reg.open(Pid::new(57), Pid::new(58)).await.unwrap();
    let path = format!("/proc/57/pipes/{}.yaml", id);
    let vfs_path = avix_core::memfs::VfsPath::parse(&path).unwrap();

    reg.close(&id).await.unwrap();
    assert!(!memfs.exists(&vfs_path).await);
}
```

---

## Step 3 — Implement

`Pipe` contains a `tokio::sync::mpsc::channel(capacity)`. `PipeRegistry` stores `HashMap<pipe_id, Pipe>` under `RwLock`. `write` to closed pipe returns `AvixError` containing `SIGPIPE`. VFS path `/proc/<source_pid>/pipes/<id>.yaml` is written on open and deleted on close.

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
git commit -m "day-22: pipes — open/write/read/close, backpressure, SIGPIPE, VFS representation"
```

## Success Criteria

- [ ] Open returns unique pipe ID
- [ ] Write + read round-trip preserves message order
- [ ] Multiple messages are FIFO
- [ ] Backpressure: third write blocks until read unblocks it
- [ ] Close → `PipeStatus::Closed`
- [ ] Write to closed pipe returns SIGPIPE error
- [ ] Pipe appears/disappears in VFS correctly
- [ ] 20+ tests pass, 0 clippy warnings

---
---

