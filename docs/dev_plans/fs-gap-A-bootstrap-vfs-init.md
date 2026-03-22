# Filesystem Gap A — Bootstrap Phase 1: VFS Tree Initialization

> **Finding:** `Runtime::bootstrap_with_root` Phase 1 logs a message but does not create any VFS
> directory skeletons. `/proc/`, `/kernel/defaults/`, and `/kernel/limits/` are never
> populated at boot, so agents that read system defaults find nothing.
>
> **Scope:** `src/bootstrap/` — Phase 1 implementation. Touches `MemFs` directly (kernel-internal
> call, no syscall layer needed). Does NOT include per-agent `/proc/<pid>/` creation — that belongs
> to finding B (agent spawn).

---

## What needs to exist after Phase 1

Phase 1 must write the following paths into `MemFs` before Phase 2 runs:

```
/proc/                              # directory anchor (empty at boot)
/proc/spawn-errors/                 # empty dir for failed spawn records
/kernel/                            # directory anchor
/kernel/defaults/                   # compiled-in system defaults
/kernel/defaults/agent.yaml         # default agent config (context window, chain limits, etc.)
/kernel/defaults/pipe.yaml          # default pipe config (buffer size, direction)
/kernel/limits/                     # dynamic limits (kernel updates at runtime)
/kernel/limits/agent.yaml           # per-agent resource ceilings
```

### Content of `/kernel/defaults/agent.yaml`

```yaml
apiVersion: avix/v1
kind: AgentDefaults
spec:
  contextWindowTokens: 64000
  maxToolChainLength: 50
  tokenTtlSecs: 3600
  renewalWindowSecs: 300
```

### Content of `/kernel/defaults/pipe.yaml`

```yaml
apiVersion: avix/v1
kind: PipeDefaults
spec:
  bufferTokens: 8192
  direction: out
```

### Content of `/kernel/limits/agent.yaml`

```yaml
apiVersion: avix/v1
kind: AgentLimits
spec:
  maxContextWindowTokens: 200000
  maxToolChainLength: 200
  maxConcurrentAgents: 100
```

---

## Step 1 — Write Tests First

Add to `crates/avix-core/tests/bootstrap.rs` (or create if it does not exist):

```rust
// ── Finding A: Phase 1 VFS tree initialization ────────────────────────────────

#[tokio::test]
async fn phase1_creates_proc_directory_anchor() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    // /proc/ anchor must exist — list should return an empty vec, not ENOENT
    let result = runtime.vfs().list(&VfsPath::parse("/proc").unwrap()).await;
    assert!(result.is_ok(), "/proc should exist after Phase 1: {:?}", result);
}

#[tokio::test]
async fn phase1_creates_kernel_defaults_agent_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/defaults/agent.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/defaults/agent.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(text.contains("contextWindowTokens"), "agent defaults must include contextWindowTokens");
    assert!(text.contains("maxToolChainLength"), "agent defaults must include maxToolChainLength");
}

#[tokio::test]
async fn phase1_creates_kernel_defaults_pipe_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/defaults/pipe.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/defaults/pipe.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(text.contains("bufferTokens"), "pipe defaults must include bufferTokens");
}

#[tokio::test]
async fn phase1_creates_kernel_limits_agent_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/limits/agent.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/limits/agent.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(text.contains("maxContextWindowTokens"), "limits must include maxContextWindowTokens");
}

#[tokio::test]
async fn phase1_creates_spawn_errors_directory() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    // Write a sentinel file to confirm the directory exists
    let sentinel = VfsPath::parse("/proc/spawn-errors/.keep").unwrap();
    let write_result = runtime.vfs().write(&sentinel, b"".to_vec()).await;
    assert!(write_result.is_ok(), "/proc/spawn-errors/ must be navigable after Phase 1");
}

#[tokio::test]
async fn phase1_runs_before_phase2() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    // /kernel/defaults/agent.yaml (written in Phase 1) must appear before Phase 2 in the boot log
    let log = runtime.boot_log();
    let phase1_idx = log.iter().position(|e| e.phase == 1).unwrap();
    let phase2_idx = log.iter().position(|e| e.phase == 2).unwrap();
    assert!(phase1_idx < phase2_idx, "Phase 1 must complete before Phase 2");
}
```

---

## Step 2 — Implementation

### 2a. Expose `vfs()` accessor on `Runtime`

```rust
impl Runtime {
    pub fn vfs(&self) -> &MemFs {
        &self.memfs
    }
}
```

### 2b. Implement `phase1::run(memfs: &MemFs)`

Create or update `src/bootstrap/phase1.rs`:

```rust
use crate::memfs::{MemFs, VfsPath};

/// Phase 1: Write the kernel VFS skeleton.
///
/// Creates directory anchors and compiles-in default/limit files so that
/// agents spawned later can read system defaults from `/kernel/defaults/`.
/// All paths written here are kernel-owned ephemeral trees — they are
/// re-created on every boot, never persisted to disk.
pub async fn run(memfs: &MemFs) {
    // /kernel/defaults/agent.yaml
    memfs.write(
        &VfsPath::parse("/kernel/defaults/agent.yaml").unwrap(),
        AGENT_DEFAULTS_YAML.as_bytes().to_vec(),
    ).await.expect("phase1: write /kernel/defaults/agent.yaml");

    // /kernel/defaults/pipe.yaml
    memfs.write(
        &VfsPath::parse("/kernel/defaults/pipe.yaml").unwrap(),
        PIPE_DEFAULTS_YAML.as_bytes().to_vec(),
    ).await.expect("phase1: write /kernel/defaults/pipe.yaml");

    // /kernel/limits/agent.yaml
    memfs.write(
        &VfsPath::parse("/kernel/limits/agent.yaml").unwrap(),
        AGENT_LIMITS_YAML.as_bytes().to_vec(),
    ).await.expect("phase1: write /kernel/limits/agent.yaml");

    // /proc/spawn-errors/.keep — anchor so the directory is listable
    memfs.write(
        &VfsPath::parse("/proc/spawn-errors/.keep").unwrap(),
        b"".to_vec(),
    ).await.expect("phase1: write /proc/spawn-errors anchor");

    tracing::info!("phase1: VFS skeleton initialised");
}

// ── Compiled-in defaults ──────────────────────────────────────────────────────

const AGENT_DEFAULTS_YAML: &str = r#"apiVersion: avix/v1
kind: AgentDefaults
spec:
  contextWindowTokens: 64000
  maxToolChainLength: 50
  tokenTtlSecs: 3600
  renewalWindowSecs: 300
"#;

const PIPE_DEFAULTS_YAML: &str = r#"apiVersion: avix/v1
kind: PipeDefaults
spec:
  bufferTokens: 8192
  direction: out
"#;

const AGENT_LIMITS_YAML: &str = r#"apiVersion: avix/v1
kind: AgentLimits
spec:
  maxContextWindowTokens: 200000
  maxToolChainLength: 200
  maxConcurrentAgents: 100
"#;
```

### 2c. Call `phase1::run` inside `Runtime::bootstrap_with_root`

In `src/bootstrap/mod.rs`, replace the Phase 1 stub with:

```rust
// Phase 1: VFS skeleton
self.boot_log.push(BootLogEntry { phase: 1, message: "VFS mount".into(), .. });
phase1::run(&self.memfs).await;
```

---

## Step 3 — Verify

```bash
cargo test --workspace
# All new phase1 tests must pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Success Criteria

- [ ] `/proc/` anchor exists after bootstrap (list returns Ok, not ENOENT)
- [ ] `/proc/spawn-errors/` directory anchor exists
- [ ] `/kernel/defaults/agent.yaml` contains `contextWindowTokens` and `maxToolChainLength`
- [ ] `/kernel/defaults/pipe.yaml` contains `bufferTokens`
- [ ] `/kernel/limits/agent.yaml` contains `maxContextWindowTokens`
- [ ] Phase 1 log entry appears before Phase 2 in the boot log
- [ ] All 6 new tests pass, 0 clippy warnings
