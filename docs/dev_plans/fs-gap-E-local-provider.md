# FS Gap E — Local Storage Provider (VFS Persistence)

> **Status:** Not started
> **Priority:** High — blocks memory service persistence across reboots
> **Depends on:** None (pure infrastructure)
> **Blocks:** memory-gap-A through G (memory records must survive reboot)
> **Affects:** `avix-core/src/memfs/` (new files), `avix-core/src/bootstrap/phase1.rs`,
>   `avix-core/src/bootstrap/phase2.rs` (new), `avix-core/src/cli/config_init.rs`

---

## Problem

`MemFs` is a `HashMap<String, Vec<u8>>` — all VFS state is lost when avix exits.
The architecture classifies `/users/`, `/crews/`, and `/services/` as
**PERSISTENT — USER/OPERATOR** and maps them to `AVIX_ROOT/data/` on disk.
`avix config init` already creates `AVIX_ROOT/data/users/<identity>/` and writes
`fstab.yaml` with the intended mounts — but fstab is never parsed and no real disk
I/O ever happens.

Memory records at `/users/<username>/memory/` need to survive avix restarts. Without
a disk-backed provider for `/users/`, they don't.

---

## Scope — Minimal, Not Full Mount System

This gap is **not** the full v0.2 mount system (cloud backends, hot-swap, `avix mount`
CLI). It implements exactly what is needed to persist the user/crew/service trees to
disk:

- `StorageProvider` trait — the interface both `MemFs` and `LocalProvider` satisfy
- `LocalProvider` — reads/writes files from `AVIX_ROOT/data/` using `std::fs`
- `VfsRouter` — replaces `Arc<MemFs>` at call sites; routes paths to the right provider
- Phase 2 bootstrap — parses fstab and mounts `/users/`, `/crews/`, `/services/`
- `config_init` — creates the additional `data/crews/` and `data/services/` directories

The `StorageProvider` trait is designed so the full v0.2 system can slot in by adding
more implementations without changing call sites.

---

## What Needs to Be Built

### 1. `StorageProvider` trait — `memfs/provider.rs`

```rust
use async_trait::async_trait;
use crate::error::AvixError;

/// A storage backend for a VFS path prefix.
/// All paths passed to these methods are relative to the mount point —
/// i.e., the mount prefix has been stripped before the call.
#[async_trait]
pub trait StorageProvider: Send + Sync + std::fmt::Debug {
    async fn read(&self, rel_path: &str) -> Result<Vec<u8>, AvixError>;
    async fn write(&self, rel_path: &str, content: Vec<u8>) -> Result<(), AvixError>;
    async fn delete(&self, rel_path: &str) -> Result<(), AvixError>;
    async fn exists(&self, rel_path: &str) -> bool;
    /// List immediate children of `rel_dir`. Returns relative names only (no path prefix).
    async fn list(&self, rel_dir: &str) -> Result<Vec<String>, AvixError>;
}
```

> `async_trait` is already in the dependency graph (used in service layer).

### 2. `MemProvider` — wrap existing `MemFs` — `memfs/mem_provider.rs`

```rust
/// Wraps MemFs so it satisfies StorageProvider.
/// Used for ephemeral trees (/proc/, /kernel/).
#[derive(Debug)]
pub struct MemProvider {
    inner: MemFs,
}

impl MemProvider {
    pub fn new() -> Self {
        Self { inner: MemFs::new() }
    }
}

#[async_trait]
impl StorageProvider for MemProvider {
    async fn read(&self, rel_path: &str) -> Result<Vec<u8>, AvixError> {
        // Prefix with "/" since VfsPath requires absolute paths
        let p = VfsPath::parse(&format!("/{}", rel_path.trim_start_matches('/')))?;
        self.inner.read(&p).await
    }
    async fn write(&self, rel_path: &str, content: Vec<u8>) -> Result<(), AvixError> {
        let p = VfsPath::parse(&format!("/{}", rel_path.trim_start_matches('/')))?;
        self.inner.write(&p, content).await
    }
    async fn delete(&self, rel_path: &str) -> Result<(), AvixError> {
        let p = VfsPath::parse(&format!("/{}", rel_path.trim_start_matches('/')))?;
        self.inner.delete(&p).await
    }
    async fn exists(&self, rel_path: &str) -> bool {
        let Ok(p) = VfsPath::parse(&format!("/{}", rel_path.trim_start_matches('/'))) else {
            return false;
        };
        self.inner.exists(&p).await
    }
    async fn list(&self, rel_dir: &str) -> Result<Vec<String>, AvixError> {
        let p = VfsPath::parse(&format!("/{}", rel_dir.trim_start_matches('/')))?;
        self.inner.list(&p).await
    }
}
```

### 3. `LocalProvider` — disk-backed — `memfs/local_provider.rs`

```rust
/// Stores files on the host filesystem under a root directory.
/// Used for persistent trees (/users/, /crews/, /services/, /etc/avix/).
#[derive(Debug, Clone)]
pub struct LocalProvider {
    /// Absolute host FS path that this provider is rooted at.
    /// e.g. "/home/alice/avix-data/data/users"
    root: std::path::PathBuf,
}

impl LocalProvider {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Result<Self, AvixError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|e| AvixError::ConfigParse(format!("LocalProvider: {e}")))?;
        Ok(Self { root })
    }

    fn full_path(&self, rel_path: &str) -> std::path::PathBuf {
        // Strip leading slash from the relative path and join under root.
        // Normalise to prevent path traversal (no ".." components).
        let rel = rel_path.trim_start_matches('/');
        self.root.join(rel)
    }
}

#[async_trait]
impl StorageProvider for LocalProvider {
    async fn read(&self, rel_path: &str) -> Result<Vec<u8>, AvixError> {
        let path = self.full_path(rel_path);
        tokio::fs::read(&path).await
            .map_err(|e| AvixError::ConfigParse(format!("ENOENT {}: {e}", path.display())))
    }

    async fn write(&self, rel_path: &str, content: Vec<u8>) -> Result<(), AvixError> {
        let path = self.full_path(rel_path);
        // Create parent directories automatically — mirrors MemFs which never errors on write
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| AvixError::ConfigParse(format!("mkdir {}: {e}", parent.display())))?;
        }
        tokio::fs::write(&path, content).await
            .map_err(|e| AvixError::ConfigParse(format!("write {}: {e}", path.display())))
    }

    async fn delete(&self, rel_path: &str) -> Result<(), AvixError> {
        let path = self.full_path(rel_path);
        tokio::fs::remove_file(&path).await
            .map_err(|e| AvixError::ConfigParse(format!("ENOENT {}: {e}", path.display())))
    }

    async fn exists(&self, rel_path: &str) -> bool {
        self.full_path(rel_path).exists()
    }

    async fn list(&self, rel_dir: &str) -> Result<Vec<String>, AvixError> {
        let path = self.full_path(rel_dir);
        let mut rd = tokio::fs::read_dir(&path).await
            .map_err(|_| AvixError::ConfigParse(format!("ENOENT {}", path.display())))?;
        let mut names = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
        if names.is_empty() {
            // Check if the dir itself exists but is empty (not an error)
            if path.is_dir() {
                return Ok(vec![]);
            }
            return Err(AvixError::ConfigParse(format!("ENOENT {}", path.display())));
        }
        Ok(names)
    }
}
```

**Path traversal safety:** `full_path()` must validate that the resolved path stays
under `self.root`:

```rust
fn full_path(&self, rel_path: &str) -> Result<std::path::PathBuf, AvixError> {
    let rel = rel_path.trim_start_matches('/');
    // Reject any path component that is ".." to prevent traversal
    for component in std::path::Path::new(rel).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(AvixError::ConfigParse(
                format!("path traversal rejected: {rel_path}")
            ));
        }
    }
    Ok(self.root.join(rel))
}
```

Change `full_path` to return `Result<PathBuf, AvixError>` and propagate the error
in each method.

### 4. `VfsRouter` — routes VFS calls to the right provider — `memfs/router.rs`

```rust
/// Mount entry: a VFS path prefix bound to a provider.
struct Mount {
    prefix: String,     // e.g. "/users" or "/proc"
    provider: Arc<dyn StorageProvider>,
}

/// Replaces Arc<MemFs> at all call sites.
/// Routes each VFS call to the longest-prefix-matching mount.
/// Falls back to the default MemProvider if no mount matches.
pub struct VfsRouter {
    mounts: tokio::sync::RwLock<Vec<Mount>>,
    default: Arc<dyn StorageProvider>,   // MemProvider — catches /proc, /kernel, etc.
}

impl VfsRouter {
    pub fn new() -> Self {
        Self {
            mounts: tokio::sync::RwLock::new(vec![]),
            default: Arc::new(MemProvider::new()),
        }
    }

    /// Register a mount point. Later mounts override earlier ones for overlapping prefixes.
    pub async fn mount(&self, prefix: &str, provider: Arc<dyn StorageProvider>) {
        let prefix = prefix.trim_end_matches('/').to_string();
        let mut mounts = self.mounts.write().await;
        // Remove any existing mount at the same prefix
        mounts.retain(|m| m.prefix != prefix);
        mounts.push(Mount { prefix, provider });
        // Sort by prefix length descending — longest prefix wins
        mounts.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
    }

    /// Find the provider for an absolute VFS path and return (provider, rel_path).
    async fn resolve<'a>(&'a self, abs_path: &str) -> (Arc<dyn StorageProvider>, String) {
        let mounts = self.mounts.read().await;
        for mount in mounts.iter() {
            if abs_path == mount.prefix
                || abs_path.starts_with(&format!("{}/", mount.prefix))
            {
                let rel = abs_path[mount.prefix.len()..].trim_start_matches('/');
                return (Arc::clone(&mount.provider), rel.to_string());
            }
        }
        // No mount matched — use default (MemProvider)
        (Arc::clone(&self.default), abs_path.trim_start_matches('/').to_string())
    }

    // ── Public API — same signature as MemFs ─────────────────────────────────

    pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError> {
        let (provider, rel) = self.resolve(path.as_str()).await;
        provider.read(&rel).await
    }

    pub async fn write(&self, path: &VfsPath, content: Vec<u8>) -> Result<(), AvixError> {
        let (provider, rel) = self.resolve(path.as_str()).await;
        provider.write(&rel, content).await
    }

    pub async fn delete(&self, path: &VfsPath) -> Result<(), AvixError> {
        let (provider, rel) = self.resolve(path.as_str()).await;
        provider.delete(&rel).await
    }

    pub async fn exists(&self, path: &VfsPath) -> bool {
        let (provider, rel) = self.resolve(path.as_str()).await;
        provider.exists(&rel).await
    }

    pub async fn list(&self, dir: &VfsPath) -> Result<Vec<String>, AvixError> {
        let (provider, rel) = self.resolve(dir.as_str()).await;
        provider.list(&rel).await
    }
}
```

> `VfsRouter` intentionally mirrors `MemFs`'s method signatures exactly so the
> type swap from `Arc<MemFs>` to `Arc<VfsRouter>` is mechanical — no logic changes
> at call sites.

### 5. Replace `Arc<MemFs>` with `Arc<VfsRouter>` throughout the codebase

Everywhere `Arc<MemFs>` appears (executor, syscall layer, service, session, etc.),
change the type to `Arc<VfsRouter>`. The method calls are identical — this is a
type substitution only.

Search for all usages:
```bash
grep -rn "Arc<MemFs>\|&MemFs\|MemFs::new" crates/avix-core/src/ --include="*.rs"
```

Expected call sites: `executor/runtime_executor.rs`, `syscall/handler.rs`,
`session/store.rs`, `process/table.rs`, `bootstrap/phase1.rs`, test helpers.

### 6. Update `bootstrap/phase1.rs` — use `VfsRouter`

Phase 1 creates the router and mounts the **ephemeral** trees from the default
`MemProvider`. Persistent tree mounts happen in Phase 2 (after config is loaded).

```rust
pub async fn run(vfs: &VfsRouter) {
    // MemProvider is already the default — /proc/ and /kernel/ fall through to it.
    // Write skeleton exactly as before (calls are identical):
    vfs.write(&VfsPath::parse("/kernel/defaults/agent-manifest.yaml").unwrap(), ...).await ...;
    vfs.write(&VfsPath::parse("/kernel/limits/agent-manifest.yaml").unwrap(), ...).await ...;
    vfs.write(&VfsPath::parse("/proc/spawn-errors/.keep").unwrap(), b"".to_vec()).await ...;
    tracing::info!("phase1: VFS skeleton initialised");
}
```

No other changes to Phase 1 logic.

### 7. New `bootstrap/phase2.rs` — parse fstab and mount persistent trees

```rust
use crate::config::fstab::{FstabConfig, FstabMount};
use crate::memfs::{VfsRouter, LocalProvider};
use std::sync::Arc;

/// Phase 2: Mount persistent VFS trees from fstab.
///
/// Called after Phase 1 (VFS skeleton) and after config files are read.
/// Adds LocalProvider mounts for /users/, /crews/, /services/, /etc/avix/.
pub async fn mount_persistent_trees(
    vfs: &VfsRouter,
    avix_root: &std::path::Path,
) -> Result<(), crate::error::AvixError> {
    // Fixed mounts — always present regardless of fstab customisation.
    // Fstab is for operator overrides; these are the invariant defaults.
    let persistent = [
        ("/etc/avix",  avix_root.join("etc")),
        ("/users",     avix_root.join("data/users")),
        ("/crews",     avix_root.join("data/crews")),
        ("/services",  avix_root.join("data/services")),
    ];

    for (vfs_prefix, disk_root) in &persistent {
        let provider = LocalProvider::new(disk_root)
            .map_err(|e| crate::error::AvixError::ConfigParse(
                format!("mount {vfs_prefix}: {e}")
            ))?;
        vfs.mount(vfs_prefix, Arc::new(provider)).await;
        tracing::info!(prefix = vfs_prefix, disk = %disk_root.display(), "mounted local provider");
    }

    Ok(())
}
```

**Why fixed mounts, not fstab-driven in this gap:**

Parsing every fstab option (encrypted, readonly, cloud backends) is the full v0.2 mount
system. This gap hard-codes the four invariant persistent trees. `fstab.yaml` is still
written by `config_init` (unchanged) and will be used by the full mount system in v0.2
without any migration.

### 8. Update `avix start` entrypoint

In the `avix start` command handler (`avix-cli/src/main.rs` or wherever `phase1::run`
is called), add the Phase 2 mount call after config is loaded:

```rust
// Existing:
let vfs = Arc::new(VfsRouter::new());
bootstrap::phase1::run(&vfs).await;

// After loading kernel.yaml and resolving AVIX_ROOT:
bootstrap::phase2::mount_persistent_trees(&vfs, &avix_root).await?;
```

### 9. Update `config_init` — create `data/crews/` and `data/services/`

```rust
// Existing (unchanged):
std::fs::create_dir_all(root.join(format!("data/users/{identity}")))...;
std::fs::create_dir_all(root.join("secrets"))...;

// Add:
std::fs::create_dir_all(root.join("data/crews"))
    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
std::fs::create_dir_all(root.join("data/services"))
    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
```

### 10. `FstabConfig` stub — `config/fstab.rs`

A minimal struct so `fstab.yaml` round-trips through serde (required for `config
reload` not to error). Full parsing is v0.2.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabMount {
    pub path: String,
    pub provider: String,
    pub config: serde_yaml::Value,
    #[serde(default)]
    pub options: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabSpec {
    pub mounts: Vec<FstabMount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub spec: FstabSpec,
}
```

---

## Path Translation Reference

| VFS path | Mounted at | Relative path | Disk path (example root `~/avix-data`) |
|----------|-----------|---------------|----------------------------------------|
| `/users/alice/memory/researcher/episodic/x.yaml` | `/users` → `data/users/` | `alice/memory/researcher/episodic/x.yaml` | `~/avix-data/data/users/alice/memory/researcher/episodic/x.yaml` |
| `/crews/analysts/memory/shared/episodic/y.yaml` | `/crews` → `data/crews/` | `analysts/memory/shared/episodic/y.yaml` | `~/avix-data/data/crews/analysts/memory/shared/episodic/y.yaml` |
| `/etc/avix/kernel.yaml` | `/etc/avix` → `etc/` | `kernel.yaml` | `~/avix-data/etc/kernel.yaml` |
| `/proc/57/status.yaml` | no mount — default MemProvider | `proc/57/status.yaml` | (in-memory only, lost on reboot — correct) |
| `/kernel/defaults/agent-manifest.yaml` | no mount — default MemProvider | `kernel/defaults/agent-manifest.yaml` | (in-memory only — correct) |

---

## TDD Test Plan

File: `crates/avix-core/src/memfs/local_provider.rs` under `#[cfg(test)]`
File: `crates/avix-core/tests/vfs_router.rs` (new integration test)

```rust
// T-FE-01: LocalProvider write creates file on disk
#[tokio::test]
async fn local_provider_write_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider.write("alice/memory/foo.yaml", b"hello".to_vec()).await.unwrap();
    assert!(dir.path().join("alice/memory/foo.yaml").exists());
}

// T-FE-02: LocalProvider write creates parent directories
#[tokio::test]
async fn local_provider_write_creates_parents() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider.write("deep/nested/dir/file.yaml", b"x".to_vec()).await.unwrap();
    assert!(dir.path().join("deep/nested/dir/file.yaml").exists());
}

// T-FE-03: LocalProvider read returns written content
#[tokio::test]
async fn local_provider_read_returns_content() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider.write("foo.yaml", b"hello world".to_vec()).await.unwrap();
    let content = provider.read("foo.yaml").await.unwrap();
    assert_eq!(content, b"hello world");
}

// T-FE-04: LocalProvider delete removes file
#[tokio::test]
async fn local_provider_delete_removes_file() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider.write("x.yaml", b"y".to_vec()).await.unwrap();
    provider.delete("x.yaml").await.unwrap();
    assert!(!dir.path().join("x.yaml").exists());
}

// T-FE-05: LocalProvider list returns immediate children only
#[tokio::test]
async fn local_provider_list_immediate_children() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    provider.write("a/foo.yaml", b"".to_vec()).await.unwrap();
    provider.write("a/bar.yaml", b"".to_vec()).await.unwrap();
    provider.write("a/sub/baz.yaml", b"".to_vec()).await.unwrap();
    let names = provider.list("a").await.unwrap();
    // Should return "foo.yaml", "bar.yaml", "sub" — NOT "sub/baz.yaml"
    assert!(names.contains(&"foo.yaml".to_string()));
    assert!(names.contains(&"bar.yaml".to_string()));
    assert!(names.contains(&"sub".to_string()));
    assert!(!names.iter().any(|n| n.contains('/')));
}

// T-FE-06: LocalProvider rejects path traversal
#[tokio::test]
async fn local_provider_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(dir.path()).unwrap();
    let result = provider.write("../escape.yaml", b"bad".to_vec()).await;
    assert!(result.is_err(), "path traversal must be rejected");
}

// T-FE-07: VfsRouter routes /users/ to LocalProvider
#[tokio::test]
async fn vfs_router_routes_users_to_local() {
    let dir = tempfile::tempdir().unwrap();
    let router = VfsRouter::new();
    let local = Arc::new(LocalProvider::new(dir.path().join("users")).unwrap());
    router.mount("/users", local).await;

    let path = VfsPath::parse("/users/alice/memory/foo.yaml").unwrap();
    router.write(&path, b"data".to_vec()).await.unwrap();

    // File should be on disk under AVIX_ROOT/users/alice/memory/foo.yaml
    assert!(dir.path().join("users/alice/memory/foo.yaml").exists());
}

// T-FE-08: VfsRouter routes /proc/ to default MemProvider (not on disk)
#[tokio::test]
async fn vfs_router_routes_proc_to_mem() {
    let dir = tempfile::tempdir().unwrap();
    let router = VfsRouter::new();
    // Only mount /users — /proc falls through to MemProvider
    router.mount("/users", Arc::new(LocalProvider::new(dir.path().join("users")).unwrap())).await;

    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    router.write(&path, b"status".to_vec()).await.unwrap();

    // Must NOT appear on disk — it's in-memory MemProvider
    assert!(!dir.path().join("proc/57/status.yaml").exists());
    // But must be readable via router
    let content = router.read(&path).await.unwrap();
    assert_eq!(content, b"status");
}

// T-FE-09: VfsRouter longest prefix wins
#[tokio::test]
async fn vfs_router_longest_prefix_wins() {
    let dir = tempfile::tempdir().unwrap();
    let router = VfsRouter::new();
    let users_root = dir.path().join("users");
    let alice_root = dir.path().join("alice-special");
    router.mount("/users", Arc::new(LocalProvider::new(&users_root).unwrap())).await;
    router.mount("/users/alice", Arc::new(LocalProvider::new(&alice_root).unwrap())).await;

    let path = VfsPath::parse("/users/alice/workspace/file.txt").unwrap();
    router.write(&path, b"alice".to_vec()).await.unwrap();
    // Should be in alice-special/, not users/alice/
    assert!(alice_root.join("workspace/file.txt").exists());
    assert!(!users_root.join("alice/workspace/file.txt").exists());
}

// T-FE-10: data survives router recreation (persistence test)
#[tokio::test]
async fn local_provider_data_survives_restart() {
    let dir = tempfile::tempdir().unwrap();

    // "First boot" — write a file
    {
        let router = VfsRouter::new();
        router.mount("/users", Arc::new(LocalProvider::new(dir.path().join("users")).unwrap())).await;
        let path = VfsPath::parse("/users/alice/memory/researcher/episodic/rec.yaml").unwrap();
        router.write(&path, b"episodic record content".to_vec()).await.unwrap();
    }

    // "Second boot" — create a fresh router, mount same disk root
    {
        let router = VfsRouter::new();
        router.mount("/users", Arc::new(LocalProvider::new(dir.path().join("users")).unwrap())).await;
        let path = VfsPath::parse("/users/alice/memory/researcher/episodic/rec.yaml").unwrap();
        let content = router.read(&path).await.unwrap();
        assert_eq!(content, b"episodic record content");
    }
}

// T-FE-11: phase2 mounts appear after mount_persistent_trees()
#[tokio::test]
async fn phase2_mounts_persistent_trees() {
    let dir = tempfile::tempdir().unwrap();
    let router = Arc::new(VfsRouter::new());
    bootstrap::phase1::run(&router).await;
    bootstrap::phase2::mount_persistent_trees(&router, dir.path()).await.unwrap();

    // /users/ should now be LocalProvider-backed
    let path = VfsPath::parse("/users/alice/workspace/test.txt").unwrap();
    router.write(&path, b"hello".to_vec()).await.unwrap();
    assert!(dir.path().join("data/users/alice/workspace/test.txt").exists());

    // /proc/ should still be in-memory
    let proc_path = VfsPath::parse("/proc/1/status.yaml").unwrap();
    router.write(&proc_path, b"ok".to_vec()).await.unwrap();
    assert!(!dir.path().join("data/proc").exists());
}
```

---

## Implementation Notes

- **`tokio::fs` for async disk I/O.** `LocalProvider` uses `tokio::fs::read`,
  `tokio::fs::write`, `tokio::fs::read_dir`, `tokio::fs::remove_file`,
  `tokio::fs::create_dir_all`. These are already available in the tokio runtime.
- **`MemFs` stays unchanged.** It is wrapped by `MemProvider` and used as the default
  in `VfsRouter`. All existing tests that create `MemFs` directly continue to work.
  Test helpers that need `VfsRouter` can call `VfsRouter::new()` (which has an
  in-memory default) without any disk setup.
- **No fstab parsing in this gap.** The four mounts (`/etc/avix`, `/users`, `/crews`,
  `/services`) are hard-coded in `phase2::mount_persistent_trees()`. The fstab file
  continues to be written and read by future tooling. Nothing breaks.
- **`/etc/avix` mount is read-only from agents** (enforced by `is_agent_writable()` in
  `VfsPath`). The `LocalProvider` itself has no read-only mode — the policy is at the
  syscall layer, exactly as the architecture doc describes.
- **List on empty LocalProvider directory returns `Ok(vec![])`,** not an error, when
  the directory exists but is empty. This differs from `MemFs::list()` which returns
  `ENOENT` for dirs with no entries. Adjust the empty check in `LocalProvider::list()`
  to check `path.is_dir()` before returning `ENOENT`. This also means `memory_svc`
  code that calls `list()` must handle empty results gracefully (it already does).
- **`config_init` creates `data/users/<identity>/`** (the specific user, not the parent
  `data/users/`). The `LocalProvider` for `/users` is rooted at `data/users/`, so when
  new users are added after init their subdirectory is created automatically by the
  first `LocalProvider::write()` call (which calls `create_dir_all` on the parent).

---

## Build Order Impact

This gap must be completed before any memory gap is implemented on a real avix
installation. In unit tests, `VfsRouter::new()` (pure in-memory default) is sufficient
— memory gap tests do not need disk persistence. The disk persistence is what matters
for production reboots.

```
fs-gap-E  →  memory-gap-A  →  memory-gap-B  →  ...
```

---

## Success Criteria

- [ ] `LocalProvider` write creates file and parent directories on disk (T-FE-01, T-FE-02)
- [ ] `LocalProvider` read returns written content (T-FE-03)
- [ ] `LocalProvider` delete removes file (T-FE-04)
- [ ] `LocalProvider` list returns only immediate children (T-FE-05)
- [ ] `LocalProvider` rejects path traversal (T-FE-06)
- [ ] `VfsRouter` routes `/users/` calls to `LocalProvider` (T-FE-07)
- [ ] `VfsRouter` routes `/proc/` to in-memory default (T-FE-08)
- [ ] `VfsRouter` longest prefix wins (T-FE-09)
- [ ] Data written in one `VfsRouter` lifetime is readable in the next (T-FE-10)
- [ ] `phase2::mount_persistent_trees()` wires correct paths (T-FE-11)
- [ ] All existing tests pass unchanged (MemFs still works via MemProvider)
- [ ] `cargo clippy --workspace -- -D warnings` passes
