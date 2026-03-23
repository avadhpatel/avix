# Memory Gap B — VFS Layout & Directory Bootstrap

> **Status:** Complete
> **Priority:** High — memory.svc reads/writes from VFS paths that don't exist yet
> **Depends on:** memory-gap-A (schema types, VFS paths), fs-gap-E (VfsRouter + LocalProvider for disk persistence)
> **Affects:** `avix-core/src/memfs/path.rs`, `avix-core/src/bootstrap/phase1.rs`, `avix-core/src/executor/runtime_executor.rs`

---

## Problem

The MemFS is a flat key-value store (`HashMap<String, Vec<u8>>`). It has no concept of
directory ownership or creation. The memory trees the spec requires:

```
/users/<username>/memory/<agent-name>/episodic/
/users/<username>/memory/<agent-name>/semantic/
/users/<username>/memory/<agent-name>/preferences/
/crews/<crew-name>/memory/shared/
/proc/services/memory/
```

...do not exist and are never initialised. Additionally:

- `VfsPath::is_agent_writable()` does not block agent direct writes to memory paths.
  Agents must never call `fs/write` on any path under `memory/` — all writes go through
  `memory.svc`.
- No `/proc/services/memory/` state directory is created at service startup.

---

## What Needs to Be Built

### 1. Update `VfsPath::is_agent_writable()` in `memfs/path.rs`

Add the memory tree to the list of agent-write-blocked paths:

```rust
pub fn is_agent_writable(&self) -> bool {
    let p = self.as_str();
    // Existing blocked paths (unchanged)
    if p.starts_with("/proc/")
        || p.starts_with("/kernel/")
        || p.starts_with("/secrets/")
        || p.starts_with("/etc/avix/")
        || p.starts_with("/bin/")
    {
        return false;
    }
    // Memory trees: agents may not call fs/write directly.
    // All memory writes go through memory.svc tools.
    if p.starts_with("/users/") && p.contains("/memory/") {
        return false;
    }
    if p.starts_with("/crews/") && p.contains("/memory/") {
        return false;
    }
    true
}
```

### 2. `MemFs::ensure_path()` — anchor directory creation

The MemFS `list()` operation works by prefix scan over keys. To make directories
"exist" (so `list()` returns something meaningful), write a `.keep` anchor file:

```rust
impl MemFs {
    /// Ensures a directory path exists in the VFS by writing a `.keep` anchor.
    /// Idempotent — safe to call multiple times.
    pub async fn ensure_dir(&self, path: &VfsPath) -> Result<(), MemFsError> {
        let keep_path = format!("{}/.keep", path.as_str().trim_end_matches('/'));
        let keep_vfs = VfsPath::parse(&keep_path)?;
        if !self.exists(&keep_vfs).await {
            self.write(&keep_vfs, b".keep".to_vec()).await?;
        }
        Ok(())
    }
}
```

### 3. `init_user_memory_tree()` — called at agent spawn

When `RuntimeExecutor` spawns and the memory block is enabled, initialise the user's
memory directories for this agent. Called from `RuntimeExecutor::spawn_with_registry()`
after VFS setup, before `SIGSTART`.

```rust
pub async fn init_user_memory_tree(
    vfs: &MemFs,
    owner: &str,
    agent_name: &str,
) -> Result<(), AvixError> {
    let base = format!("/users/{}/memory/{}", owner, agent_name);
    for subdir in &["episodic", "semantic", "preferences", "grants"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    }
    // Index subdirectories (for BM25 and vectors — built by memory.svc, not agents)
    for subdir in &["episodic/index", "semantic/index"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    }
    Ok(())
}
```

### 4. `init_crew_memory_tree()` — called when crew is created

```rust
pub async fn init_crew_memory_tree(vfs: &MemFs, crew_name: &str) -> Result<(), AvixError> {
    let base = format!("/crews/{}/memory/shared", crew_name);
    for subdir in &["episodic", "semantic", "episodic/index", "semantic/index"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    }
    Ok(())
}
```

### 5. `/proc/services/memory/` state directory in Phase 1 bootstrap

Add to `bootstrap/phase1.rs` alongside the existing `/proc/spawn-errors/` anchor:

```rust
// Runtime state for memory.svc
vfs.ensure_dir(&VfsPath::parse("/proc/services/memory/").unwrap()).await?;
vfs.ensure_dir(&VfsPath::parse("/proc/services/memory/agents/").unwrap()).await?;
```

### 6. `memory_svc_status_path()` and `memory_agent_stats_path()`

Helpers for the `/proc/services/memory/` runtime state paths:

```rust
pub fn memory_svc_status_path() -> &'static str {
    "/proc/services/memory/status.yaml"
}

pub fn memory_agent_stats_path(agent_name: &str) -> String {
    format!("/proc/services/memory/agents/{}/stats.yaml", agent_name)
}

pub fn memory_agent_grants_path(agent_name: &str, grant_id: &str) -> String {
    format!("/proc/services/memory/agents/{}/grants/{}.yaml", agent_name, grant_id)
}
```

### 7. `MemorySvcStatus` — `/proc/services/memory/status.yaml`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySvcStatus {
    pub healthy: bool,
    pub total_episodic_records: u64,
    pub total_semantic_records: u64,
    pub active_session_grants: u32,
    pub updated_at: DateTime<Utc>,
}
```

### 8. `MemoryAgentStats` — `/proc/services/memory/agents/<name>/stats.yaml`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAgentStats {
    pub agent_name: String,
    pub episodic_record_count: u32,
    pub semantic_record_count: u32,
    pub last_write_at: Option<DateTime<Utc>>,
    pub last_retrieval_at: Option<DateTime<Utc>>,
}
```

---

## TDD Test Plan

File: `crates/avix-core/src/memfs/path.rs` under `#[cfg(test)]` (extend existing tests)

```rust
// T-MB-01: memory tree paths are NOT agent-writable
#[test]
fn memory_tree_not_agent_writable() {
    let paths = [
        "/users/alice/memory/researcher/episodic/2026-03-22T14:30:00Z-abc.yaml",
        "/users/alice/memory/researcher/semantic/project-alpha.yaml",
        "/users/alice/memory/researcher/preferences/user-model.yaml",
        "/crews/analysts/memory/shared/episodic/some.yaml",
    ];
    for p in &paths {
        let vfs_path = VfsPath::parse(p).unwrap();
        assert!(
            !vfs_path.is_agent_writable(),
            "expected non-writable by agent: {p}"
        );
    }
}

// T-MB-02: workspace paths remain agent-writable
#[test]
fn workspace_paths_still_writable() {
    let path = VfsPath::parse("/users/alice/workspace/report.md").unwrap();
    assert!(path.is_agent_writable());
}

// T-MB-03: ensure_dir is idempotent
#[tokio::test]
async fn ensure_dir_idempotent() {
    let vfs = MemFs::new();
    let dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    vfs.ensure_dir(&dir).await.unwrap();
    vfs.ensure_dir(&dir).await.unwrap();  // second call must not error
    let entries = vfs.list(&dir).await.unwrap();
    assert!(!entries.is_empty(), "expected .keep anchor");
}

// T-MB-04: init_user_memory_tree creates all required subdirs
#[tokio::test]
async fn init_user_memory_tree_creates_dirs() {
    let vfs = MemFs::new();
    init_user_memory_tree(&vfs, "alice", "researcher").await.unwrap();
    for dir in &[
        "/users/alice/memory/researcher/episodic",
        "/users/alice/memory/researcher/semantic",
        "/users/alice/memory/researcher/preferences",
        "/users/alice/memory/researcher/grants",
        "/users/alice/memory/researcher/episodic/index",
        "/users/alice/memory/researcher/semantic/index",
    ] {
        let p = VfsPath::parse(dir).unwrap();
        assert!(
            vfs.exists(&VfsPath::parse(&format!("{}/.keep", dir)).unwrap()).await,
            "expected dir anchor at {dir}"
        );
    }
}

// T-MB-05: phase1 bootstrap creates /proc/services/memory/
#[tokio::test]
async fn phase1_creates_memory_svc_proc_dirs() {
    let vfs = MemFs::new();
    run_phase1_bootstrap(&vfs).await.unwrap();
    assert!(
        vfs.exists(&VfsPath::parse("/proc/services/memory/agents/.keep").unwrap()).await,
        "expected /proc/services/memory/agents/ to be created at bootstrap"
    );
}

// T-MB-06: MemorySvcStatus round-trips through YAML
#[test]
fn memory_svc_status_round_trips() {
    let status = MemorySvcStatus {
        healthy: true,
        total_episodic_records: 1234,
        total_semantic_records: 567,
        active_session_grants: 2,
        updated_at: Utc::now(),
    };
    let yaml = serde_yaml::to_string(&status).unwrap();
    let parsed: MemorySvcStatus = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(parsed.total_episodic_records, 1234);
}
```

---

## Implementation Notes

- `is_agent_writable()` checks `p.starts_with("/users/") && p.contains("/memory/")`. This
  correctly blocks `alice/memory/` while leaving `alice/workspace/` and `alice/home/`
  writable. The substring check is safe because valid VFS paths cannot have `..`.
- The MemFS `list()` method already does prefix filtering — `ensure_dir()` just guarantees
  the prefix produces at least one result (the `.keep` key), consistent with how
  `/proc/spawn-errors/` is bootstrapped in Phase 1 today.
- `init_user_memory_tree()` is called per-agent at spawn, not per-user at user creation.
  This is intentional: memory trees are created on demand when an agent with memory
  capability first spawns, matching the Unix `mkdir -p` on first use pattern.
- Crew memory initialisation (`init_crew_memory_tree()`) is called from the crew creation
  path, not at agent spawn, since multiple agents share one crew tree.

---

## Success Criteria

- [ ] Memory tree paths block agent `fs/write` calls (T-MB-01)
- [ ] Workspace paths remain writable (T-MB-02)
- [ ] `ensure_dir()` is idempotent (T-MB-03)
- [ ] `init_user_memory_tree()` creates all required subdirs (T-MB-04)
- [ ] Phase 1 bootstrap creates `/proc/services/memory/` (T-MB-05)
- [ ] `MemorySvcStatus` round-trips through YAML (T-MB-06)
- [ ] `cargo clippy --workspace -- -D warnings` passes
