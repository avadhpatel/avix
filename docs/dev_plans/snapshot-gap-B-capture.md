# Snapshot Gap B — Capture and VFS Persistence

> **Status:** Not started
> **Priority:** High — SIGSAVE handler is a stub; auto/manual snapshots not implemented
> **Depends on:** snapshot-gap-A (SnapshotFile / SnapshotSpec structs)
> **Affects:** `avix-core/src/snapshot/capture.rs`, `avix-core/src/executor/runtime_executor.rs`, `avix-core/src/syscall/domain/snap_.rs`

---

## Problem

Snapshot capture does not actually work:

1. **SIGSAVE handler is a no-op.** `runtime_executor.rs` line 419–423 logs `"SIGSAVE received; snapshot not yet implemented"` and does nothing.
2. **No VFS write.** Snapshots are only held in `SnapshotStore` (in-memory `HashMap`). The spec location `/users/<username>/snapshots/<name>.yaml` is never written.
3. **No context serialisation.** `SnapshotSpec.contextSummary`, `contextTokenCount`, `memory`, `environment`, `checksum` are not computed from the running executor state.
4. **Syscall stubs.** `snap/save`, `snap/list`, and `snap/delete` all return hard-coded mock responses.
5. **Auto-snapshot not implemented.** `AgentManifest.spec.snapshot.autoSnapshotIntervalSec` is resolved (Gap D wrote it to `/proc/<pid>/resolved.yaml`) but never acted on.

---

## What Needs to Be Built

### `Snapshot::capture_from(...)` — build `SnapshotFile` from executor state

```rust
/// Parameters needed to capture a snapshot from a live executor.
pub struct CaptureParams<'a> {
    pub agent_name: &'a str,
    pub pid: u32,
    pub username: &'a str,
    pub goal: &'a str,
    pub message_history: &'a [SnapshotMessage],
    pub temperature: f32,
    pub granted_tools: &'a [String],    // for capability_token derivation
    pub trigger: SnapshotTrigger,
    pub captured_by: CapturedBy,
    pub memory: SnapshotMemory,
    pub pending_requests: Vec<PendingRequest>,
    pub open_pipes: Vec<SnapshotPipe>,
}

pub fn capture(params: CaptureParams<'_>) -> SnapshotFile {
    let captured_at = chrono::Utc::now();
    let name = SnapshotFile::make_name(params.agent_name, &captured_at);

    // Estimate context token count (characters / 4 is a rough proxy)
    let context_chars: usize = params.message_history.iter()
        .map(|m| m.content.len())
        .sum();
    let context_token_count = (context_chars / 4).max(1) as u32;

    // Build a context summary from the last user/assistant message pair
    let context_summary = build_context_summary(params.message_history);

    // Capability token signature = sha256 of joined tool names (stable fingerprint)
    let capability_token = sha256_hex(&params.granted_tools.join(","));

    let spec = SnapshotSpec {
        goal: params.goal.to_string(),
        context_summary,
        context_token_count,
        memory: params.memory,
        pending_requests: params.pending_requests,
        pipes: params.open_pipes,
        environment: SnapshotEnvironment {
            temperature: params.temperature,
            capability_token: format!("sha256:{capability_token}"),
        },
        checksum: String::new(), // populated after serialisation below
    };

    // Serialise spec → compute checksum → embed
    let mut file = SnapshotFile::new(
        SnapshotMetadata {
            name,
            agent_name: params.agent_name.to_string(),
            source_pid: params.pid,
            captured_at,
            captured_by: params.captured_by,
            trigger: params.trigger,
        },
        spec,
    );
    let checksum = compute_checksum(&file);
    file.spec.checksum = format!("sha256:{checksum}");
    file
}
```

**`build_context_summary`** — returns last ~200 chars of the most recent assistant turn, or a
generic string if no messages exist.

**`compute_checksum`** — serialise the file to YAML with `checksum: ""` (zeroed), compute SHA-256
of the resulting bytes, return hex string.

```rust
fn compute_checksum(file: &SnapshotFile) -> String {
    let mut zeroed = file.clone();
    zeroed.spec.checksum = String::new();
    let yaml = zeroed.to_yaml().unwrap_or_default();
    sha256_hex(yaml.as_bytes())
}

fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    use sha2::{Sha256, Digest};
    let hash = Sha256::digest(data.as_ref());
    hex::encode(hash)
}
```

> `sha2` and `hex` are already in the dependency graph via the IPC/auth code; do not add new deps.

### `RuntimeExecutor` — SIGSAVE handler

Replace the stub in `runtime_executor.rs`:

```rust
"SIGSAVE" => {
    self.capture_and_write_snapshot(SnapshotTrigger::Sigsave, CapturedBy::Kernel).await;
}
```

New method:

```rust
async fn capture_and_write_snapshot(&self, trigger: SnapshotTrigger, captured_by: CapturedBy) {
    let params = CaptureParams {
        agent_name: &self.agent_name,
        pid: self.pid.as_u32(),
        username: &self.spawned_by,
        goal: &self.goal,
        message_history: &self.conversation_history,
        temperature: self.resolved_temperature(),
        granted_tools: &self.token.granted_tools,
        trigger,
        captured_by,
        memory: SnapshotMemory::default(), // fill from memory.svc after memory-gap-C lands
        pending_requests: vec![],          // Gap C: fill from in-flight requests
        open_pipes: vec![],               // Gap C: fill from pipe registry
    };
    let snap = capture(params);
    let vfs_path = snap.vfs_path(&self.spawned_by);
    match snap.to_yaml() {
        Ok(yaml) => {
            if let Ok(path) = VfsPath::parse(&vfs_path) {
                if let Err(e) = self.vfs.write(&path, yaml.into_bytes()).await {
                    tracing::warn!(pid = self.pid.as_u32(), path = vfs_path, err = ?e, "snapshot write failed");
                } else {
                    tracing::info!(pid = self.pid.as_u32(), path = vfs_path, "snapshot written");
                }
            }
        }
        Err(e) => tracing::warn!(pid = self.pid.as_u32(), err = ?e, "snapshot serialisation failed"),
    }
}
```

`resolved_temperature()` reads from the executor's cached `ResolvedConfig` (already present after Gap D).

### Auto-snapshot task

At the end of `RuntimeExecutor::spawn_with_registry()`, start a background task if
`resolved.snapshot.auto_snapshot_interval_sec > 0`:

```rust
if self.resolved_snapshot.auto_snapshot_interval_sec > 0 {
    let interval = std::time::Duration::from_secs(
        self.resolved_snapshot.auto_snapshot_interval_sec as u64
    );
    // store handle to cancel on executor drop
    let _handle = tokio::spawn(auto_snapshot_loop(
        self.weak_handle(),  // weak ref to avoid keeping executor alive
        interval,
    ));
}
```

`auto_snapshot_loop` fires `capture_and_write_snapshot(SnapshotTrigger::Auto, CapturedBy::Kernel)` on each interval tick.

### Syscall `snap/save` — manual snapshot

Replace the stub in `snap_.rs`:

```rust
pub async fn save(ctx: &SyscallContext, params: Value) -> SyscallResult {
    let pid = params.get("pid").and_then(|v| v.as_u64())
        .ok_or_else(|| SyscallError::Einval("missing pid".into()))? as u32;

    // Trigger capture on the target executor (via jobs/executor registry)
    ctx.trigger_snapshot(pid, SnapshotTrigger::Manual, CapturedBy::Agent(ctx.caller_pid))
        .await
        .map_err(|e| SyscallError::Enoent(format!("snapshot failed: {e}")))?;

    let name = format!("{}-manual-{}", ctx.agent_name_for(pid)?, chrono::Utc::now().format("%H%M%S"));
    Ok(json!({ "snapshot_id": name, "pid": pid }))
}
```

### Syscall `snap/list` — list from VFS

```rust
pub async fn list(ctx: &SyscallContext, params: Value) -> SyscallResult {
    let username = ctx.username();
    let vfs_path = VfsPath::parse(&format!("/users/{username}/snapshots/")).unwrap();
    let entries = ctx.vfs.list(&vfs_path).await.unwrap_or_default();
    let names: Vec<_> = entries.iter()
        .filter(|e| e.ends_with(".yaml"))
        .map(|e| e.trim_end_matches(".yaml"))
        .collect();
    Ok(json!({ "snapshots": names }))
}
```

### Syscall `snap/delete` — delete from VFS

```rust
pub async fn delete(ctx: &SyscallContext, params: Value) -> SyscallResult {
    let snapshot_id = params.get("snapshot_id").and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing snapshot_id".into()))?;
    let username = ctx.username();
    let path = VfsPath::parse(&format!("/users/{username}/snapshots/{snapshot_id}.yaml")).unwrap();
    ctx.vfs.delete(&path).await
        .map_err(|e| SyscallError::Enoent(format!("delete failed: {e}")))?;
    Ok(json!({ "snapshot_id": snapshot_id, "deleted": true }))
}
```

---

## TDD Test Plan

File: `crates/avix-core/tests/snapshot.rs` (new integration test file)

```rust
// T-SB-01: capture() builds a valid SnapshotFile with checksum
#[test]
fn snapshot_capture_produces_valid_file() {
    let messages = vec![
        SnapshotMessage { role: "user".into(), content: "Research quantum computing".into() },
        SnapshotMessage { role: "assistant".into(), content: "I'll start by searching...".into() },
    ];
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "Research quantum computing",
        message_history: &messages,
        temperature: 0.7,
        granted_tools: &["fs/read".to_string(), "llm/complete".to_string()],
        trigger: SnapshotTrigger::Sigsave,
        captured_by: CapturedBy::Kernel,
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    });
    assert_eq!(snap.kind, "Snapshot");
    assert_eq!(snap.metadata.agent_name, "researcher");
    assert_eq!(snap.metadata.trigger, SnapshotTrigger::Sigsave);
    assert!(snap.spec.context_token_count > 0);
    assert!(!snap.spec.checksum.is_empty());
    assert!(snap.spec.checksum.starts_with("sha256:"));
}

// T-SB-02: checksum changes when content changes
#[test]
fn snapshot_checksum_detects_tampering() {
    let snap1 = capture(minimal_capture_params("goal A"));
    let snap2 = capture(minimal_capture_params("goal B"));
    assert_ne!(snap1.spec.checksum, snap2.spec.checksum);
}

// T-SB-03: vfs_path() is correct
#[test]
fn snapshot_vfs_path_correct() {
    let snap = capture(minimal_capture_params("test"));
    let path = snap.vfs_path("alice");
    assert!(path.starts_with("/users/alice/snapshots/researcher-"));
    assert!(path.ends_with(".yaml"));
}

// T-SB-04: SIGSAVE writes snapshot to VFS
#[tokio::test]
async fn sigsave_writes_snapshot_to_vfs() {
    let (executor, vfs) = spawn_test_executor("alice").await;
    executor.deliver_signal("SIGSAVE").await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // Snapshot file should exist in VFS
    let entries = vfs.list(&VfsPath::parse("/users/alice/snapshots/").unwrap()).await.unwrap();
    assert!(!entries.is_empty(), "expected snapshot file in VFS after SIGSAVE");
}

// T-SB-05: snap/save syscall returns snapshot_id
#[tokio::test]
async fn syscall_snap_save_returns_id() {
    let (executor, _vfs) = spawn_test_executor("alice").await;
    let result = executor.call_syscall("snap/save", json!({ "pid": executor.pid() })).await;
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val.get("snapshot_id").is_some());
}

// T-SB-06: snap/list returns names of written snapshots
#[tokio::test]
async fn syscall_snap_list_returns_snapshots() {
    let (executor, _vfs) = spawn_test_executor("alice").await;
    executor.deliver_signal("SIGSAVE").await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let result = executor.call_syscall("snap/list", json!({})).await.unwrap();
    let snaps = result["snapshots"].as_array().unwrap();
    assert!(!snaps.is_empty());
}
```

---

## Implementation Notes

- The `VfsPath::list()` operation may not exist yet — check `memfs.rs` and add it if needed (list children of a directory-like path prefix).
- For the SIGSAVE handler, the executor already has `self.vfs` from Gap D. Use it directly.
- `resolved_temperature()` reads from the cached `resolved.environment.temperature` that Gap D already writes to the process table.
- The auto-snapshot loop needs a weak executor reference to avoid keeping the executor alive after it exits. Use a `Weak<RwLock<ExecutorState>>` or a channel-based cancellation token.
- `snap/save` as a syscall requires `SyscallContext` to have access to the executor registry to trigger capture on a given PID. If that's not feasible yet, `snap/save` can instead write directly via VFS (the executor calls capture on itself).
- Gap C (restore) depends on the `checksum` field being correctly populated here.

---

## Success Criteria

- [ ] `capture()` produces a valid `SnapshotFile` with non-empty checksum (T-SB-01)
- [ ] Checksum changes when content changes (T-SB-02)
- [ ] `vfs_path()` produces correct path (T-SB-03)
- [ ] SIGSAVE writes file to VFS (T-SB-04)
- [ ] `snap/save` syscall returns `snapshot_id` (T-SB-05)
- [ ] `snap/list` returns written snapshot names (T-SB-06)
- [ ] `cargo clippy --workspace -- -D warnings` passes
