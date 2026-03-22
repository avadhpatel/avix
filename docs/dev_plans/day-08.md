# Day 8 — MemFS / VFS — Virtual Filesystem

> **Goal:** Build the in-memory virtual filesystem (MemFS) — the VFS that backs `/proc/`, `/kernel/`, and serves generated-on-demand files for agent status, service state, and runtime observations. Target: <50µs read latency.

---

## Pre-flight: Verify Day 7

```bash
cargo test --workspace     # all Day 7 router tests pass
grep -r "ServiceRegistry"  crates/avix-core/src/router/
grep -r "inject_caller"    crates/avix-core/src/router/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod memfs;`

```
src/memfs/
├── mod.rs
├── vfs.rs       ← MemFs struct with read/write/list/watch
├── node.rs      ← VfsNode (file or directory)
└── path.rs      ← VfsPath (validated absolute path)
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/memfs.rs`:

```rust
use avix_core::memfs::{MemFs, VfsPath};

// ── Basic read/write ──────────────────────────────────────────────────────────

#[tokio::test]
async fn write_and_read_file() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    fs.write(&path, b"status: running\n".to_vec()).await.unwrap();
    let content = fs.read(&path).await.unwrap();
    assert_eq!(content, b"status: running\n");
}

#[tokio::test]
async fn read_missing_file_returns_err() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/99/status.yaml").unwrap();
    assert!(fs.read(&path).await.is_err());
}

#[tokio::test]
async fn overwrite_replaces_content() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/kernel/defaults/agent.yaml").unwrap();
    fs.write(&path, b"v1".to_vec()).await.unwrap();
    fs.write(&path, b"v2".to_vec()).await.unwrap();
    assert_eq!(fs.read(&path).await.unwrap(), b"v2");
}

// ── Directory listing ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_directory() {
    let fs = MemFs::new();
    fs.write(&VfsPath::parse("/proc/57/status.yaml").unwrap(), b"".to_vec()).await.unwrap();
    fs.write(&VfsPath::parse("/proc/57/resolved.yaml").unwrap(), b"".to_vec()).await.unwrap();

    let entries = fs.list(&VfsPath::parse("/proc/57").unwrap()).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.contains(&"status.yaml".to_string()));
    assert!(entries.contains(&"resolved.yaml".to_string()));
}

#[tokio::test]
async fn list_missing_directory_returns_err() {
    let fs = MemFs::new();
    assert!(fs.list(&VfsPath::parse("/proc/999").unwrap()).await.is_err());
}

// ── Delete ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_file() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    fs.write(&path, b"x".to_vec()).await.unwrap();
    fs.delete(&path).await.unwrap();
    assert!(fs.read(&path).await.is_err());
}

// ── Exists ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn exists_returns_correct_values() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    assert!(!fs.exists(&path).await);
    fs.write(&path, b"x".to_vec()).await.unwrap();
    assert!(fs.exists(&path).await);
}

// ── Path validation ───────────────────────────────────────────────────────────

#[test]
fn vfs_path_requires_absolute() {
    assert!(VfsPath::parse("relative/path").is_err());
    assert!(VfsPath::parse("/absolute/path").is_ok());
}

#[test]
fn vfs_path_rejects_traversal() {
    assert!(VfsPath::parse("/proc/../etc/passwd").is_err());
}

#[test]
fn vfs_path_parent() {
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    assert_eq!(path.parent().unwrap().as_str(), "/proc/57");
}

// ── /proc/gateway/ generated view ─────────────────────────────────────────────

#[tokio::test]
async fn proc_gateway_connections_file_exists_after_write() {
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/gateway/connections.yaml").unwrap();
    fs.write(&path, b"connections: []".to_vec()).await.unwrap();
    assert!(fs.exists(&path).await);
}

// ── Concurrent access ─────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_writes_to_different_paths() {
    use std::sync::Arc;
    let fs = Arc::new(MemFs::new());
    let mut handles = Vec::new();

    for i in 0..50u32 {
        let f = Arc::clone(&fs);
        handles.push(tokio::spawn(async move {
            let path = VfsPath::parse(&format!("/proc/{i}/status.yaml")).unwrap();
            f.write(&path, format!("pid: {i}").into_bytes()).await.unwrap();
        }));
    }
    for h in handles { h.await.unwrap(); }

    for i in 0..50u32 {
        let path = VfsPath::parse(&format!("/proc/{i}/status.yaml")).unwrap();
        assert!(fs.exists(&path).await);
    }
}
```

---

## Step 3 — Implement

`MemFs` backed by `Arc<RwLock<HashMap<String, Vec<u8>>>>`. `VfsPath` is a validated wrapper around `String` that enforces absoluteness and rejects `..` traversal.

---

## Step 4 — Benchmark Stub

```rust
// benches/memfs.rs — target: <50µs read
fn bench_memfs_read(c: &mut Criterion) {
    // pre-write a file, then bench read
}
```

---

## Step 5 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-08: MemFS VFS with path validation, concurrent R/W, <50µs target"
```

## Success Criteria

- [ ] 15+ tests pass
- [ ] Path traversal (`..`) rejected
- [ ] Relative paths rejected
- [ ] Concurrent writes to 50 different paths — all readable after join
- [ ] `/proc/gateway/` path works
- [ ] Benchmark stub present
- [ ] 0 clippy warnings

---
---

