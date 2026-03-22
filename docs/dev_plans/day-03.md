# Day 3 — ProcessTable with TDD

> **Goal:** Build the in-memory process table — the kernel's authoritative registry of every running agent and service. Backed by `Arc<RwLock<HashMap>>`, targeting <5µs read/write operations, with full concurrency correctness.

---

## Pre-flight: Verify Day 2

```bash
cargo test --workspace
# Expected: all Day 2 type tests pass (30+)

# Confirm the key types exist
grep -r "pub struct Pid"       crates/avix-core/src/
grep -r "pub struct IpcAddr"   crates/avix-core/src/
grep -r "pub enum Modality"    crates/avix-core/src/
grep -r "pub struct ToolName"  crates/avix-core/src/
grep -r "CapabilityToolMap"    crates/avix-core/src/

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings
```

All checks must pass before writing new code.

---

## Step 1 — Extend the Module Tree

Add to `crates/avix-core/src/lib.rs`:

```rust
pub mod error;
pub mod types;
pub mod process;   // NEW
```

Create `crates/avix-core/src/process/mod.rs`:

```rust
pub mod table;
pub mod entry;

pub use table::ProcessTable;
pub use entry::{ProcessEntry, ProcessKind, ProcessStatus};
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/process_table.rs`:

```rust
use avix_core::process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable};
use avix_core::types::Pid;
use std::sync::Arc;

fn make_agent_entry(pid: u32, name: &str) -> ProcessEntry {
    ProcessEntry {
        pid:    Pid::new(pid),
        name:   name.to_string(),
        kind:   ProcessKind::Agent,
        status: ProcessStatus::Running,
        parent: None,
        spawned_by_user: "alice".to_string(),
    }
}

fn make_service_entry(pid: u32, name: &str) -> ProcessEntry {
    ProcessEntry {
        pid:    Pid::new(pid),
        name:   name.to_string(),
        kind:   ProcessKind::Service,
        status: ProcessStatus::Running,
        parent: None,
        spawned_by_user: "system".to_string(),
    }
}

// ── Basic CRUD ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn insert_and_lookup_by_pid() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    let entry = table.get(Pid::new(57)).await.unwrap();
    assert_eq!(entry.name, "researcher");
}

#[tokio::test]
async fn lookup_missing_pid_returns_none() {
    let table = ProcessTable::new();
    assert!(table.get(Pid::new(99)).await.is_none());
}

#[tokio::test]
async fn remove_entry() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.remove(Pid::new(57)).await;
    assert!(table.get(Pid::new(57)).await.is_none());
}

#[tokio::test]
async fn remove_nonexistent_is_noop() {
    let table = ProcessTable::new();
    // Must not panic
    table.remove(Pid::new(999)).await;
}

// ── Update status ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_status() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.set_status(Pid::new(57), ProcessStatus::Paused).await.unwrap();
    let entry = table.get(Pid::new(57)).await.unwrap();
    assert_eq!(entry.status, ProcessStatus::Paused);
}

#[tokio::test]
async fn update_status_missing_pid_returns_err() {
    let table = ProcessTable::new();
    let result = table.set_status(Pid::new(99), ProcessStatus::Paused).await;
    assert!(result.is_err());
}

// ── Filtering ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_all() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.insert(make_agent_entry(58, "writer")).await;
    table.insert(make_service_entry(2, "router")).await;
    let all = table.list_all().await;
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn list_agents_only() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.insert(make_service_entry(2, "router")).await;
    let agents = table.list_by_kind(ProcessKind::Agent).await;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "researcher");
}

#[tokio::test]
async fn list_by_parent() {
    let table = ProcessTable::new();
    let mut child = make_agent_entry(58, "child");
    child.parent = Some(Pid::new(57));
    table.insert(make_agent_entry(57, "parent")).await;
    table.insert(child).await;

    let children = table.list_children(Pid::new(57)).await;
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].pid, Pid::new(58));
}

#[tokio::test]
async fn list_by_status() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "running-agent")).await;
    let mut paused = make_agent_entry(58, "paused-agent");
    paused.status = ProcessStatus::Paused;
    table.insert(paused).await;

    let running = table.list_by_status(ProcessStatus::Running).await;
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].name, "running-agent");
}

// ── Lookup by name ────────────────────────────────────────────────────────────

#[tokio::test]
async fn find_by_name() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    let found = table.find_by_name("researcher").await.unwrap();
    assert_eq!(found.pid, Pid::new(57));
}

#[tokio::test]
async fn find_by_name_missing_returns_none() {
    let table = ProcessTable::new();
    assert!(table.find_by_name("ghost").await.is_none());
}

// ── Concurrency correctness ───────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_inserts_all_visible() {
    let table = Arc::new(ProcessTable::new());
    let mut handles = Vec::new();

    for i in 0..100u32 {
        let t = Arc::clone(&table);
        handles.push(tokio::spawn(async move {
            t.insert(make_agent_entry(i + 100, &format!("agent-{i}"))).await;
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(table.list_all().await.len(), 100);
}

#[tokio::test]
async fn concurrent_reads_do_not_block_each_other() {
    let table = Arc::new(ProcessTable::new());
    table.insert(make_agent_entry(57, "researcher")).await;

    let mut handles = Vec::new();
    for _ in 0..50 {
        let t = Arc::clone(&table);
        handles.push(tokio::spawn(async move {
            t.get(Pid::new(57)).await.is_some()
        }));
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    assert!(results.iter().all(|r| *r.as_ref().unwrap()));
}

// ── Count ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn count_is_accurate() {
    let table = ProcessTable::new();
    assert_eq!(table.count().await, 0);
    table.insert(make_agent_entry(57, "a")).await;
    assert_eq!(table.count().await, 1);
    table.remove(Pid::new(57)).await;
    assert_eq!(table.count().await, 0);
}
```

---

## Step 3 — Implement

**`src/process/entry.rs`**

```rust
use crate::types::Pid;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    Agent,
    Service,
    Kernel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Paused,
    Waiting,
    Stopped,
    Crashed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid:             Pid,
    pub name:            String,
    pub kind:            ProcessKind,
    pub status:          ProcessStatus,
    pub parent:          Option<Pid>,
    pub spawned_by_user: String,
}
```

**`src/process/table.rs`**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::AvixError;
use crate::types::Pid;
use super::entry::{ProcessEntry, ProcessKind, ProcessStatus};

#[derive(Debug, Default)]
pub struct ProcessTable {
    inner: Arc<RwLock<HashMap<u32, ProcessEntry>>>,
}

impl ProcessTable {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, entry: ProcessEntry) {
        self.inner.write().await.insert(entry.pid.as_u32(), entry);
    }

    pub async fn remove(&self, pid: Pid) {
        self.inner.write().await.remove(&pid.as_u32());
    }

    pub async fn get(&self, pid: Pid) -> Option<ProcessEntry> {
        self.inner.read().await.get(&pid.as_u32()).cloned()
    }

    pub async fn set_status(&self, pid: Pid, status: ProcessStatus) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        match guard.get_mut(&pid.as_u32()) {
            Some(e) => { e.status = status; Ok(()) }
            None    => Err(AvixError::InvalidPid(pid.to_string())),
        }
    }

    pub async fn list_all(&self) -> Vec<ProcessEntry> {
        self.inner.read().await.values().cloned().collect()
    }

    pub async fn list_by_kind(&self, kind: ProcessKind) -> Vec<ProcessEntry> {
        self.inner.read().await.values()
            .filter(|e| e.kind == kind)
            .cloned().collect()
    }

    pub async fn list_by_status(&self, status: ProcessStatus) -> Vec<ProcessEntry> {
        self.inner.read().await.values()
            .filter(|e| e.status == status)
            .cloned().collect()
    }

    pub async fn list_children(&self, parent: Pid) -> Vec<ProcessEntry> {
        self.inner.read().await.values()
            .filter(|e| e.parent == Some(parent))
            .cloned().collect()
    }

    pub async fn find_by_name(&self, name: &str) -> Option<ProcessEntry> {
        self.inner.read().await.values()
            .find(|e| e.name == name)
            .cloned()
    }

    pub async fn count(&self) -> usize {
        self.inner.read().await.len()
    }
}
```

Add `futures` to dev-dependencies in `crates/avix-core/Cargo.toml` (for the concurrency test):

```toml
[dev-dependencies]
tempfile.workspace = true
tokio    = { workspace = true, features = ["test-util"] }
futures  = "0.3"
```

---

## Step 4 — Add Performance Benchmark Stub

Create `crates/avix-core/benches/process_table.rs`:

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use avix_core::process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable};
use avix_core::types::Pid;

fn entry(pid: u32) -> ProcessEntry {
    ProcessEntry {
        pid:    Pid::new(pid),
        name:   format!("agent-{pid}"),
        kind:   ProcessKind::Agent,
        status: ProcessStatus::Running,
        parent: None,
        spawned_by_user: "alice".to_string(),
    }
}

fn bench_process_table(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Pre-populate table
    let table = ProcessTable::new();
    rt.block_on(async {
        for i in 0..1000u32 {
            table.insert(entry(i)).await;
        }
    });

    c.bench_function("process_table_get", |b| {
        b.iter(|| {
            rt.block_on(async { table.get(Pid::new(42)).await })
        });
    });
    // Target: < 5µs
}

criterion_group!(benches, bench_process_table);
criterion_main!(benches);
```

Add to `crates/avix-core/Cargo.toml`:

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }

[[bench]]
name    = "process_table"
harness = false
```

---

## Step 5 — Verify

```bash
cargo test --workspace
# Expected: all Day 3 tests pass (15+ new tests)

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings

cargo fmt --check
# Expected: exit 0

# Optional: run bench to check <5µs target
cargo bench --bench process_table 2>/dev/null | grep process_table_get
```

---

## Commit

```bash
git add -A
git commit -m "day-03: ProcessTable with concurrent RwLock, all operations tested"
```

---

## Success Criteria

- [ ] 15+ tests pass
- [ ] Concurrent insert (100 goroutines) — all entries visible after join
- [ ] Concurrent reads — 50 simultaneous reads return correct results
- [ ] `set_status` on missing PID returns `Err`
- [ ] `remove` on non-existent PID is a no-op (no panic)
- [ ] Benchmark exists (target <5µs for `get`)
- [ ] 0 clippy warnings
