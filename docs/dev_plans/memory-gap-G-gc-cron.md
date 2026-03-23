# Memory Gap G — GC and Cron Tasks

> **Status:** Not started
> **Priority:** Low — system runs without GC; episodic records accumulate until this is implemented
> **Depends on:** memory-gap-C (service tools), memory-gap-E (vector index, for reindex job)
> **Affects:** `avix-core/src/cron_svc/`, `avix-core/src/memory_svc/gc.rs` (new)

---

## Problem

1. Episodic records older than `maxRetentionDays` are never deleted.
2. Expired session-scoped `MemoryGrant` records are never cleaned up.
3. Vector indexes are never rebuilt when the embedding model changes.
4. The `CronScheduler` exists but is not started at kernel boot, so no scheduled tasks
   run.

---

## What Needs to Be Built

### 1. `memory_svc/gc.rs` — GC logic

```rust
pub struct GcReport {
    pub records_deleted: u64,
    pub bytes_freed: u64,
    pub grants_pruned: u32,
}

/// Delete episodic records older than retention_days for all agents.
pub async fn gc_episodic_records(
    vfs: &MemFs,
    users: &[String],
    retention_days: u32,
) -> Result<GcReport, AvixError> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut report = GcReport::default();

    for user in users {
        // List all agent dirs under /users/<user>/memory/
        let memory_base = format!("/users/{}/memory", user);
        let agent_dirs = vfs.list(&VfsPath::parse(&memory_base).unwrap()).await.unwrap_or_default();

        for agent_dir in agent_dirs {
            let episodic_dir = format!("{}/episodic", agent_dir);
            let entries = vfs.list(&VfsPath::parse(&episodic_dir).unwrap()).await.unwrap_or_default();

            for entry in entries.iter().filter(|e| e.ends_with(".yaml")) {
                if let Ok(record) = store::read_record(vfs, entry).await {
                    if record.metadata.pinned {
                        continue;  // pinned records are never GC'd
                    }
                    if record.metadata.created_at < cutoff {
                        if let Ok(bytes) = vfs.read(&VfsPath::parse(entry).unwrap()).await {
                            report.bytes_freed += bytes.len() as u64;
                        }
                        vfs.delete(&VfsPath::parse(entry).unwrap()).await.ok();
                        report.records_deleted += 1;
                    }
                }
            }
        }
    }

    Ok(report)
}

/// Prune expired session-scoped MemoryGrant records from /proc/services/memory/.
pub async fn prune_expired_grants(vfs: &MemFs) -> Result<u32, AvixError> {
    let grant_root = "/proc/services/memory/agents";
    let agent_dirs = vfs.list(&VfsPath::parse(grant_root).unwrap()).await.unwrap_or_default();
    let mut pruned = 0u32;
    let now = Utc::now();

    for agent_dir in agent_dirs {
        let grants_dir = format!("{}/grants", agent_dir);
        let grants = vfs.list(&VfsPath::parse(&grants_dir).unwrap()).await.unwrap_or_default();
        for grant_path in grants.iter().filter(|e| e.ends_with(".yaml")) {
            if let Ok(grant) = load_grant(vfs, grant_path).await {
                // Prune if: session-scoped with non-null expiresAt that has passed
                let expired = grant.spec.expires_at
                    .map(|exp| exp < now)
                    .unwrap_or(false);
                if expired {
                    vfs.delete(&VfsPath::parse(grant_path).unwrap()).await.ok();
                    pruned += 1;
                }
            }
        }
    }
    Ok(pruned)
}
```

### 2. `memory_svc/reindex.rs` — reindex logic

```rust
pub struct ReindexReport {
    pub records_reindexed: u64,
    pub records_skipped: u64,
    pub duration_ms: u64,
}

/// Rebuild stale vector indexes for all agents.
/// Only processes records where spec.index.vectorModel != current_model.
pub async fn reindex_delta(
    vfs: &MemFs,
    index_manager: &IndexManager,
    users: &[String],
    current_model: &str,
) -> Result<ReindexReport, AvixError> {
    let start = std::time::Instant::now();
    let mut report = ReindexReport::default();

    for user in users {
        let agent_dirs = list_agent_dirs(vfs, user).await;
        for agent_dir in agent_dirs {
            let agent_name = agent_dir.split('/').last().unwrap_or("");
            for mem_type in &["episodic", "semantic"] {
                let dir = format!("{}/{}", agent_dir, mem_type);
                let records = store::list_records(vfs, &dir, user, agent_name).await
                    .unwrap_or_default();
                for record in records {
                    if record.spec.index.vector_model.as_deref() == Some(current_model) {
                        report.records_skipped += 1;
                        continue;
                    }
                    // Re-embed (no caller token in cron context; use system token)
                    match index_manager.index_record_with_system_token(&record, user, agent_name).await {
                        Ok(_) => report.records_reindexed += 1,
                        Err(e) => {
                            tracing::warn!(err = ?e, record_id = %record.metadata.id, "reindex failed");
                        }
                    }
                }
            }
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;
    Ok(report)
}
```

### 3. Wire GC and reindex into `CronScheduler`

The `CronScheduler` exists but is not started at kernel boot. Wire it up in the
kernel startup sequence and register the two memory tasks.

In `bootstrap/phase2.rs` (or wherever the kernel services are started):

```rust
// Start cron scheduler
let scheduler = Arc::new(CronScheduler::new());

// memory-gc-daily: runs at 03:00 UTC every day
scheduler.add_job(CronJob {
    name: "memory-gc-daily".into(),
    schedule: "0 3 * * *".parse().unwrap(),
    handler: Box::new({
        let vfs = Arc::clone(&vfs);
        let memory_config = Arc::clone(&kernel_config.memory);
        let users = users_list.clone();
        move || {
            let vfs = Arc::clone(&vfs);
            let memory_config = Arc::clone(&memory_config);
            let users = users.clone();
            Box::pin(async move {
                let report = gc_episodic_records(
                    &vfs, &users, memory_config.episodic.max_retention_days
                ).await?;
                let grants_pruned = prune_expired_grants(&vfs).await?;
                tracing::info!(
                    records_deleted = report.records_deleted,
                    bytes_freed = report.bytes_freed,
                    grants_pruned,
                    "memory-gc-daily complete"
                );
                Ok(())
            })
        }
    }),
    missed_run_policy: MissedRunPolicy::Skip,
}).await;

// memory-reindex-weekly: runs at 04:00 UTC every Sunday
scheduler.add_job(CronJob {
    name: "memory-reindex-weekly".into(),
    schedule: "0 4 * * 0".parse().unwrap(),
    handler: Box::new({
        let vfs = Arc::clone(&vfs);
        let index_manager = Arc::clone(&index_manager);
        let users = users_list.clone();
        let current_model = kernel_config.models.default_embedding_model.clone();
        move || {
            Box::pin(async move {
                let report = reindex_delta(&vfs, &index_manager, &users, &current_model).await?;
                tracing::info!(
                    records_reindexed = report.records_reindexed,
                    records_skipped = report.records_skipped,
                    duration_ms = report.duration_ms,
                    "memory-reindex-weekly complete"
                );
                Ok(())
            })
        }
    }),
    missed_run_policy: MissedRunPolicy::Skip,
}).await;

scheduler.start().await;
```

### 4. Check `CronScheduler` cron expression support

The existing `CronScheduler` uses basic cron parsing. Verify it supports the
`"0 3 * * *"` form (minute, hour, day, month, weekday). If it only supports 5-field
cron, this is already correct. If it uses 6-field (with seconds), prepend `0`:
`"0 0 3 * * *"`.

---

## TDD Test Plan

File: `crates/avix-core/tests/memory_gc.rs`

```rust
// T-MG-01: GC deletes episodic records older than retention_days
#[tokio::test]
async fn gc_deletes_old_records() {
    let vfs = MemFs::new();
    init_user_memory_tree(&vfs, "alice", "researcher").await.unwrap();

    // Write an old record (31 days ago)
    let old_created_at = Utc::now() - chrono::Duration::days(31);
    let old_record = make_test_episodic_record_at("alice", "researcher", old_created_at);
    let old_path = MemoryRecord::vfs_path_episodic("alice", "researcher", &old_created_at, &old_record.metadata.id);
    store::write_record(&vfs, &old_path, &old_record).await.unwrap();

    // Write a recent record
    let new_record = make_test_episodic_record("alice", "researcher");
    let new_path = MemoryRecord::vfs_path_episodic("alice", "researcher", &Utc::now(), &new_record.metadata.id);
    store::write_record(&vfs, &new_path, &new_record).await.unwrap();

    let report = gc_episodic_records(&vfs, &["alice".to_string()], 30).await.unwrap();
    assert_eq!(report.records_deleted, 1);
    assert!(vfs.exists(&VfsPath::parse(&new_path).unwrap()).await, "recent record must survive GC");
    assert!(!vfs.exists(&VfsPath::parse(&old_path).unwrap()).await, "old record must be deleted");
}

// T-MG-02: GC never deletes pinned records
#[tokio::test]
async fn gc_spares_pinned_records() {
    let vfs = MemFs::new();
    let old_date = Utc::now() - chrono::Duration::days(60);
    let mut old_pinned = make_test_episodic_record_at("alice", "researcher", old_date);
    old_pinned.metadata.pinned = true;
    // write to VFS...
    let report = gc_episodic_records(&vfs, &["alice".to_string()], 30).await.unwrap();
    assert_eq!(report.records_deleted, 0, "pinned records must never be GC'd");
}

// T-MG-03: prune_expired_grants removes expired session grants
#[tokio::test]
async fn prune_removes_expired_grants() {
    let vfs = MemFs::new();
    // Write a session grant with expiresAt in the past
    let expired_grant = make_test_grant_expired();
    // ... write to /proc/services/memory/agents/writer/grants/
    let pruned = prune_expired_grants(&vfs).await.unwrap();
    assert_eq!(pruned, 1);
}

// T-MG-04: reindex skips records with matching vectorModel
#[tokio::test]
async fn reindex_skips_current_model() {
    // Write a record with spec.index.vectorModel = "current-model"
    // Run reindex with current_model = "current-model"
    // Verify records_skipped == 1, records_reindexed == 0
}

// T-MG-05: reindex processes stale records
#[tokio::test]
async fn reindex_processes_stale_records() {
    // Write a record with spec.index.vectorModel = "old-model"
    // Run reindex with current_model = "new-model"
    // Verify records_reindexed == 1
    // Verify spec.index.vectorModel updated to "new-model"
}

// T-MG-06: CronScheduler fires memory-gc-daily handler
#[tokio::test]
async fn cron_fires_gc_daily() {
    let scheduler = build_test_scheduler_with_memory_jobs().await;
    // Manually trigger the job by advancing the scheduler clock
    scheduler.trigger_job("memory-gc-daily").await;
    // Assert GC ran (check log or report)
}
```

---

## Implementation Notes

- The GC job runs as `user: svc-memory-gc`. This service user does not need a real
  agent manifest or LLM capability — it has direct VFS access via the kernel path.
  In practice, the cron handler is a Rust async closure, not an LLM agent. The YAML
  cron spec (from the memory spec) is documentation of intent; the implementation is
  a native Rust function registered with `CronScheduler`.
- The `reindex_delta` job uses a "system token" (a kernel-issued token with
  `llm:embedding` capability) for the `llm/embed` calls, since there is no calling
  agent. Add a `CapabilityToken::system_embedding_token()` factory method.
- `MissedRunPolicy::Skip` is correct for both jobs — if the system was down when the
  GC or reindex was scheduled, skip and wait for the next scheduled run. Catching up
  is not worth the burst load.
- Check whether `CronScheduler::start()` already exists or needs to be added. If the
  scheduler is event-driven, wire it into a `tokio::spawn` background task at boot.

---

## Success Criteria

- [ ] GC deletes episodic records older than `retention_days` (T-MG-01)
- [ ] GC never deletes pinned records (T-MG-02)
- [ ] Expired session grants pruned (T-MG-03)
- [ ] Reindex skips up-to-date records (T-MG-04)
- [ ] Reindex processes stale records and updates `vectorModel` (T-MG-05)
- [ ] CronScheduler fires GC job handler (T-MG-06)
- [ ] `cargo clippy --workspace -- -D warnings` passes
