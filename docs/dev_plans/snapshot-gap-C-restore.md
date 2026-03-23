# Snapshot Gap C — Restore Logic

> **Status:** Not started
> **Priority:** Medium — depends on gaps A and B; needed for crash recovery and agent cloning
> **Depends on:** snapshot-gap-A (schema), snapshot-gap-B (capture + VFS writes)
> **Affects:** `avix-core/src/syscall/domain/snap_.rs`, `avix-core/src/executor/runtime_executor.rs`

---

## Problem

The `snap/restore` syscall handler is a stub that returns `{"restored": true}` without doing anything:

```rust
pub fn restore(_ctx: &SyscallContext, params: Value) -> SyscallResult {
    let snapshot_id = ...;
    Ok(json!({ "snapshot_id": snapshot_id, "restored": true }))
}
```

The spec restore sequence requires:

1. Read snapshot YAML from VFS at `/users/<username>/snapshots/<name>.yaml`
2. Verify checksum — abort with integrity error if mismatch
3. Issue a fresh `CapabilityToken` from the original tool set (not reuse the snapshotted token)
4. Restore agent context (goal + message history → resume conversation)
5. Re-issue all `pendingRequests` with `status: in-flight` to the kernel
6. Reconnect open `pipes` — if the target agent is still running, reconnect; otherwise deliver `SIGPIPE`

None of this is implemented.

---

## What Needs to Be Built

### `verify_checksum()` — integrity check before restore

```rust
/// Returns `Ok(())` if the snapshot's embedded checksum matches its content.
/// Returns `Err` with a description if there is a mismatch.
pub fn verify_checksum(file: &SnapshotFile) -> Result<(), AvixError> {
    let stored = file.spec.checksum.strip_prefix("sha256:")
        .ok_or_else(|| AvixError::ConfigParse("invalid checksum format".into()))?;
    let computed = compute_checksum(file);   // same helper as capture.rs
    if stored != computed {
        return Err(AvixError::ConfigParse(format!(
            "snapshot integrity check failed for '{}': stored={stored} computed={computed}",
            file.metadata.name
        )));
    }
    Ok(())
}
```

### `RestoreResult` — output of a restore operation

```rust
pub struct RestoreResult {
    pub snapshot_name: String,
    pub agent_name: String,
    /// PIDs of any pending requests that were re-issued.
    pub reissued_requests: Vec<String>,
    /// Pipe IDs that were successfully reconnected.
    pub reconnected_pipes: Vec<String>,
    /// Pipe IDs for which the target was gone; a SIGPIPE will be sent.
    pub sigpipe_pipes: Vec<String>,
}
```

### `RuntimeExecutor::restore_from_snapshot()` — core restore logic

```rust
pub async fn restore_from_snapshot(
    &mut self,
    snapshot_name: &str,
    vfs: &MemFs,
) -> Result<RestoreResult, AvixError> {
    // 1. Read YAML from VFS
    let path = VfsPath::parse(&format!(
        "/users/{}/snapshots/{}.yaml",
        self.spawned_by, snapshot_name
    )).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    let bytes = vfs.read(&path).await
        .map_err(|e| AvixError::ConfigParse(format!("snapshot not found: {e}")))?;
    let yaml = String::from_utf8(bytes)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let file = SnapshotFile::from_str(&yaml)?;

    // 2. Verify checksum
    verify_checksum(&file)?;

    // 3. Issue a fresh CapabilityToken from the original tool set
    //    The snapshotted token is used only to derive the original tool list.
    let original_tools = derive_tools_from_capability_token(&file.spec.environment.capability_token);
    let fresh_token = CapabilityToken::test_token(
        &original_tools.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    );
    self.token = fresh_token;

    // 4. Restore agent context
    //    Rebuild message_history from the snapshot's context summary as a synthetic message.
    //    Full message history is not stored in the snapshot (only contextSummary).
    self.goal = file.spec.goal.clone();
    if !file.spec.context_summary.is_empty() {
        self.conversation_history = vec![
            SnapshotMessage {
                role: "assistant".into(),
                content: format!(
                    "[Restored from snapshot '{}']\n\nContext at capture:\n{}",
                    file.metadata.name,
                    file.spec.context_summary
                ),
            }
        ];
    }

    // 5. Re-issue pending requests
    let mut reissued = vec![];
    for req in &file.spec.pending_requests {
        if req.status == "in-flight" {
            tracing::info!(
                pid = self.pid.as_u32(),
                request_id = %req.request_id,
                "re-issuing in-flight request from snapshot"
            );
            // Re-issue: enqueue the tool call via the executor's tool dispatch
            // (exact mechanism depends on job/tool infrastructure from ipc-gap-D/E)
            reissued.push(req.request_id.clone());
        }
    }

    // 6. Reconnect or SIGPIPE open pipes
    let mut reconnected = vec![];
    let mut sigpipe = vec![];
    for pipe in &file.spec.pipes {
        if pipe.state == "open" {
            // Check if the pipe target is still running (via pipe registry)
            // If pipe registry is not yet available, all pipes get SIGPIPE
            // (conservative: no data loss, agent handles reconnect)
            tracing::info!(
                pid = self.pid.as_u32(),
                pipe_id = %pipe.pipe_id,
                "pipe was open at snapshot; target may be gone — delivering SIGPIPE"
            );
            sigpipe.push(pipe.pipe_id.clone());
        }
    }

    tracing::info!(
        pid = self.pid.as_u32(),
        snapshot = %file.metadata.name,
        reissued = ?reissued,
        sigpipe = ?sigpipe,
        "restore complete"
    );

    Ok(RestoreResult {
        snapshot_name: file.metadata.name.clone(),
        agent_name: file.metadata.agent_name.clone(),
        reissued_requests: reissued,
        reconnected_pipes: reconnected,
        sigpipe_pipes: sigpipe,
    })
}
```

> **Note on pipe reconnection:** Full pipe reconnection requires the pipe registry from
> `ipc-gap-E`. In this gap, pipes always result in SIGPIPE on restore. A follow-up gap
> can upgrade this once the pipe registry is available.

### `derive_tools_from_capability_token()` — recover tool list

The snapshotted token is stored as `"sha256:<hex>"` — it is a fingerprint, not the full token.
On restore, the tool set must come from one of two sources:

1. **Preferred:** Re-read the resolved config from VFS (`/proc/<pid>/resolved.yaml` →
   `granted_tools`) if it still exists.
2. **Fallback:** The `granted_tools` list is stored in `SnapshotMeta` directly (add a
   `granted_tools: Vec<String>` field to `SnapshotSpec.environment` or to the metadata).

For now, add `granted_tools: Vec<String>` to `SnapshotEnvironment`:

```rust
pub struct SnapshotEnvironment {
    pub temperature: f32,
    pub capability_token: String,  // sha256 fingerprint
    /// Original tool list at capture time; used to issue a fresh token on restore.
    #[serde(default)]
    pub granted_tools: Vec<String>,
}
```

Then `derive_tools_from_capability_token()` simply returns `file.spec.environment.granted_tools`.

> Update `snapshot-gap-A` accordingly when implementing (add `granted_tools` to `SnapshotEnvironment`).

### Syscall `snap/restore` — full restore handler

```rust
pub async fn restore(ctx: &SyscallContext, params: Value) -> SyscallResult {
    let snapshot_id = params.get("snapshot_id").and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing snapshot_id".into()))?;

    let result = ctx.executor
        .restore_from_snapshot(snapshot_id, ctx.vfs)
        .await
        .map_err(|e| SyscallError::Eio(format!("restore failed: {e}")))?;

    Ok(json!({
        "snapshot_id": result.snapshot_name,
        "restored": true,
        "reissued_requests": result.reissued_requests,
        "reconnected_pipes": result.reconnected_pipes,
        "sigpipe_pipes": result.sigpipe_pipes,
    }))
}
```

---

## TDD Test Plan

File: `crates/avix-core/tests/snapshot.rs` (extend the test file from Gap B)

```rust
// T-SC-01: verify_checksum passes for a freshly captured snapshot
#[test]
fn snapshot_verify_checksum_passes() {
    let snap = capture(minimal_capture_params("test goal"));
    assert!(verify_checksum(&snap).is_ok());
}

// T-SC-02: verify_checksum fails for a tampered snapshot
#[test]
fn snapshot_verify_checksum_detects_tampering() {
    let mut snap = capture(minimal_capture_params("test goal"));
    snap.spec.goal = "TAMPERED".into();  // mutate after capture (checksum now stale)
    assert!(verify_checksum(&snap).is_err());
}

// T-SC-03: restore reads from VFS and rebuilds context
#[tokio::test]
async fn snapshot_restore_rebuilds_context() {
    let vfs = MemFs::new();
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "Research quantum computing",
        message_history: &[],
        temperature: 0.7,
        granted_tools: &["fs/read".to_string()],
        trigger: SnapshotTrigger::Manual,
        captured_by: CapturedBy::User(1001),
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    });

    // Write to VFS
    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes()).await.unwrap();

    // Restore
    let mut executor = make_test_executor("alice", 99).await;
    let result = executor.restore_from_snapshot("researcher-20260315-0741", &vfs).await;
    assert!(result.is_ok());
    let r = result.unwrap();
    assert_eq!(r.agent_name, "researcher");
    assert_eq!(executor.goal(), "Research quantum computing");
}

// T-SC-04: restore aborts on checksum mismatch
#[tokio::test]
async fn snapshot_restore_aborts_on_bad_checksum() {
    let vfs = MemFs::new();
    let mut snap = capture(minimal_capture_params("goal"));
    snap.spec.goal = "TAMPERED".into();  // corrupt the content
    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes()).await.unwrap();

    let mut executor = make_test_executor("alice", 99).await;
    let result = executor.restore_from_snapshot(&snap.metadata.name, &vfs).await;
    assert!(result.is_err(), "expected error on checksum mismatch");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("integrity"), "expected integrity error, got: {msg}");
}

// T-SC-05: restore issues a fresh CapabilityToken (not reusing snapshotted one)
#[tokio::test]
async fn snapshot_restore_issues_fresh_token() {
    let vfs = MemFs::new();
    let snap = capture(CaptureParams {
        granted_tools: &["fs/read".to_string(), "llm/complete".to_string()],
        ..minimal_capture_params_owned("goal")
    });
    let original_tools = snap.spec.environment.granted_tools.clone();
    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes()).await.unwrap();

    let mut executor = make_test_executor("alice", 99).await;
    executor.restore_from_snapshot(&snap.metadata.name, &vfs).await.unwrap();

    // The restored token must cover the original tools
    for tool in &original_tools {
        assert!(
            executor.token().granted_tools.contains(tool),
            "fresh token missing tool '{tool}'"
        );
    }
}

// T-SC-06: pending requests are reported in RestoreResult
#[tokio::test]
async fn snapshot_restore_reports_pending_requests() {
    let vfs = MemFs::new();
    let snap = capture(CaptureParams {
        pending_requests: vec![
            PendingRequest { request_id: "req-abc".into(), resource: "tool".into(),
                             name: "web".into(), status: "in-flight".into() },
        ],
        ..minimal_capture_params_owned("goal")
    });
    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes()).await.unwrap();

    let mut executor = make_test_executor("alice", 99).await;
    let result = executor.restore_from_snapshot(&snap.metadata.name, &vfs).await.unwrap();
    assert!(result.reissued_requests.contains(&"req-abc".to_string()));
}

// T-SC-07: snap/restore syscall returns expected JSON
#[tokio::test]
async fn syscall_snap_restore_returns_json() {
    let (executor, vfs) = spawn_test_executor("alice").await;
    // First write a snapshot via SIGSAVE
    executor.deliver_signal("SIGSAVE").await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let snapshots = vfs.list(&VfsPath::parse("/users/alice/snapshots/").unwrap()).await.unwrap();
    assert!(!snapshots.is_empty());
    let snap_name = snapshots[0].trim_end_matches(".yaml").to_string();

    // Now restore it
    let result = executor.call_syscall("snap/restore", json!({ "snapshot_id": snap_name }))
        .await.unwrap();
    assert_eq!(result["restored"], true);
    assert_eq!(result["snapshot_id"], snap_name);
}
```

---

## Implementation Notes

- `verify_checksum()` must use the same `compute_checksum()` helper as `capture.rs`. Put both in `snapshot/checksum.rs` (a new sub-module) so Gap B and Gap C can both import without circular deps.
- **Full message history is not persisted** in the snapshot — only `contextSummary`. This is intentional: the snapshot is a recovery checkpoint, not a full replay log. On restore, the agent continues from the summary. If full replay is needed, that is a separate concern (episodic memory store).
- **Pipe reconnection** in this gap always results in SIGPIPE. The comment in the implementation makes clear it is intentionally conservative and will be upgraded when the pipe registry (`ipc-gap-E`) is available.
- `granted_tools` added to `SnapshotEnvironment` is a small addendum to Gap A. Note it in the Gap A plan's success criteria if not already there (update that doc or just add it in code alongside Gap C).
- `ctx.executor` reference in the syscall: the current syscall context (`SyscallContext`) may not carry an executor reference. The `snap/restore` syscall should instead be a **kernel tool** registered by `RuntimeExecutor` itself (Category 2 tool per ADR-04), so the executor can handle it directly.

---

## Success Criteria

- [ ] `verify_checksum` passes for freshly captured snapshot (T-SC-01)
- [ ] `verify_checksum` detects tampered content (T-SC-02)
- [ ] `restore_from_snapshot` rebuilds goal and context (T-SC-03)
- [ ] Restore aborts with integrity error on checksum mismatch (T-SC-04)
- [ ] Fresh `CapabilityToken` issued covering original tools (T-SC-05)
- [ ] `pendingRequests` reported in `RestoreResult` (T-SC-06)
- [ ] `snap/restore` syscall returns correct JSON (T-SC-07)
- [ ] `cargo clippy --workspace -- -D warnings` passes
