# Filesystem Gap B — Agent Spawn: Write `/proc/<pid>/status.yaml` and `resolved.yaml`

> **Finding:** `RuntimeExecutor::spawn_with_registry` creates the in-memory agent state but
> never writes anything to the VFS. The spec requires `/proc/<pid>/status.yaml` and
> `/proc/<pid>/resolved.yaml` to exist for any running agent. Without them, the kernel and
> other agents cannot observe agent state via the filesystem.
>
> **Scope:** `src/executor/runtime_executor.rs` (spawn path) and `src/syscall/domain/proc_.rs`
> (kernel/proc/spawn syscall, Day 21). Both paths must write these files.
> The VFS handle is injected via the existing `with_vfs()` builder.

---

## What must be written at spawn

### `/proc/<pid>/status.yaml`

Serialized from `ProcessEntry`. Schema (from `docs/spec/agent-status.md`):

```yaml
apiVersion: avix/v1
kind: AgentStatus
metadata:
  pid: 57
  name: researcher
spec:
  status: running            # running | paused | stopped | completed
  goal: "Research Q3 data"
  spawnedBy: alice
  sessionId: sess-abc
  grantedTools:
    - fs/read
    - llm/complete
  tokenExpiresAt: 2026-03-22T12:00:00Z   # omit if None
  toolChainDepth: 0
  contextTokensUsed: 0
```

### `/proc/<pid>/resolved.yaml`

The merged final configuration this agent runs under. For now this is a stub that records
the token grants and spawn parameters — full defaults/limits merging is deferred.

```yaml
apiVersion: avix/v1
kind: Resolved
metadata:
  pid: 57
  name: researcher
spec:
  contextWindowTokens: 64000    # from /kernel/defaults/agent.yaml
  maxToolChainLength: 50        # from /kernel/defaults/agent.yaml
  tokenTtlSecs: 3600
  grantedTools:
    - fs/read
    - llm/complete
```

---

## Step 1 — Write Tests First

Add to `crates/avix-core/tests/runtime_executor.rs`:

```rust
// ── Finding B: VFS writes at agent spawn ─────────────────────────────────────

#[tokio::test]
async fn spawn_writes_status_yaml_to_vfs() {
    let handler = Arc::new(KernelResourceHandler::new(TEST_KEY.to_vec()));
    let vfs = Arc::new(MemFs::new());
    let (executor, _reg) = spawn_with_signed_token(600, &["fs/read", "llm/complete"]).await;
    let executor = executor
        .with_resource_handler(handler)
        .with_vfs(Arc::clone(&vfs));

    let path = VfsPath::parse("/proc/600/status.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/600/status.yaml must exist after spawn when VFS is attached"
    );
}

#[tokio::test]
async fn spawn_status_yaml_contains_pid_and_name() {
    let vfs = Arc::new(MemFs::new());
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(601),
        agent_name: "my-researcher".into(),
        goal: "do research".into(),
        spawned_by: "alice".into(),
        session_id: "sess-601".into(),
        token: CapabilityToken::test_token(&["fs/read"]),
    };
    let _executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let raw = vfs.read(&VfsPath::parse("/proc/601/status.yaml").unwrap()).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("601"), "status.yaml must contain pid 601");
    assert!(text.contains("my-researcher"), "status.yaml must contain agent name");
    assert!(text.contains("alice"), "status.yaml must contain spawnedBy");
    assert!(text.contains("running"), "status.yaml must show status: running");
}

#[tokio::test]
async fn spawn_writes_resolved_yaml_to_vfs() {
    let vfs = Arc::new(MemFs::new());
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(602),
        agent_name: "writer".into(),
        goal: "write report".into(),
        spawned_by: "kernel".into(),
        session_id: "sess-602".into(),
        token: CapabilityToken::test_token(&["fs/read", "fs/write"]),
    };
    let _executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let path = VfsPath::parse("/proc/602/resolved.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/602/resolved.yaml must exist after spawn when VFS is attached"
    );
    let raw = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("fs/read"), "resolved.yaml must list granted tools");
    assert!(text.contains("fs/write"), "resolved.yaml must list all granted tools");
}

#[tokio::test]
async fn spawn_without_vfs_does_not_panic() {
    // No VFS attached — spawn must succeed silently without writing anything
    let (executor, _reg) = spawn_with_caps(603, &["fs/read"]).await;
    assert_eq!(executor.pid().as_u32(), 603);
}
```

---

## Step 2 — Implementation

### 2a. Write VFS files at the end of `spawn_with_registry`

In `src/executor/runtime_executor.rs`, add a private helper:

```rust
/// Write /proc/<pid>/status.yaml and /proc/<pid>/resolved.yaml to VFS if a
/// VFS handle is attached. Called once at spawn and after every status change.
async fn write_proc_files(&self) {
    let vfs = match &self.vfs {
        Some(v) => v,
        None => return,
    };
    let pid = self.pid.as_u32();

    // status.yaml
    let status_yaml = format!(
        "apiVersion: avix/v1\nkind: AgentStatus\nmetadata:\n  pid: {pid}\n  name: {name}\nspec:\n  status: running\n  goal: {goal:?}\n  spawnedBy: {spawned_by}\n  sessionId: {session_id}\n  grantedTools:\n{tools}  toolChainDepth: 0\n  contextTokensUsed: 0\n",
        pid = pid,
        name = self.agent_name,
        goal = self.goal,
        spawned_by = self.spawned_by,
        session_id = self.session_id,
        tools = self.token.granted_tools.iter()
            .map(|t| format!("    - {t}\n"))
            .collect::<String>(),
    );
    if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/status.yaml")) {
        let _ = vfs.write(&path, status_yaml.into_bytes()).await;
    }

    // resolved.yaml — stub: echo back token grants + compiled-in defaults
    let resolved_yaml = format!(
        "apiVersion: avix/v1\nkind: Resolved\nmetadata:\n  pid: {pid}\n  name: {name}\nspec:\n  contextWindowTokens: 64000\n  maxToolChainLength: 50\n  tokenTtlSecs: 3600\n  grantedTools:\n{tools}",
        pid = pid,
        name = self.agent_name,
        tools = self.token.granted_tools.iter()
            .map(|t| format!("    - {t}\n"))
            .collect::<String>(),
    );
    if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/resolved.yaml")) {
        let _ = vfs.write(&path, resolved_yaml.into_bytes()).await;
    }
}
```

Call `self.write_proc_files().await;` at the end of `spawn_with_registry`, after
`executor.refresh_tool_list()`:

```rust
executor.refresh_tool_list();
executor.write_proc_files().await;  // ← add this
Ok(executor)
```

Note: `with_vfs()` is called **after** `spawn_with_registry` returns, so `write_proc_files`
must also be called from `with_vfs()`:

```rust
pub fn with_vfs(mut self, vfs: Arc<MemFs>) -> Self {
    self.vfs = Some(vfs);
    // Write proc files now that VFS is attached
    // Use a blocking approach since this is a sync builder:
    // spawn a task or call a sync write stub.
    // Simplest: make with_vfs async, or call a sync-compatible path.
    self
}
```

**Preferred approach:** Make `with_vfs` trigger the write via a `tokio::spawn` or keep the
existing sync signature and write in the test setup explicitly. The cleanest solution given
the existing builder pattern is to add a separate async `init_proc_files()` method that
tests call after `with_vfs`:

```rust
pub async fn init_proc_files(&self) {
    self.write_proc_files().await;
}
```

Tests then call:
```rust
let executor = RuntimeExecutor::spawn_with_registry(params, registry).await.unwrap()
    .with_vfs(Arc::clone(&vfs));
executor.init_proc_files().await;
```

### 2b. `kernel/proc/spawn` syscall (Day 21 scope)

When the Day-21 syscall handler calls the equivalent of `spawn_with_registry`, it must
also call `init_proc_files()`. The handler has access to both the process table and MemFS,
so it writes the files directly after inserting the `ProcessEntry`.

---

## Step 3 — Verify

```bash
cargo test --workspace
# All 4 new spawn VFS tests must pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Success Criteria

- [ ] `/proc/<pid>/status.yaml` exists in VFS after spawn when VFS is attached
- [ ] `status.yaml` contains `pid`, `name`, `spawnedBy`, `status: running`
- [ ] `/proc/<pid>/resolved.yaml` exists in VFS after spawn when VFS is attached
- [ ] `resolved.yaml` lists all `grantedTools` from the token
- [ ] Spawn without VFS attached succeeds silently (no panic)
- [ ] 4 new tests pass, 0 clippy warnings
