# restart-gap-A — Boot-time Crash Recovery

> Goal: On every `avix start`, atomically repair stale persistence records left by the
> previous run before any agents or ATP clients can observe inconsistent state.

---

## Problem

After a daemon crash or clean shutdown, the `InvocationStore` (redb) and
`PersistentSessionStore` (redb) contain records whose in-memory state is permanently lost:

| Record status | Why it's stale | Correct post-restart status |
|---|---|---|
| `InvocationStatus::Running` | Executor task is dead | `Killed` (exit_reason: "interrupted_at_shutdown") |
| `InvocationStatus::Paused` | Atomic pause flag lost | `Killed` (exit_reason: "interrupted_at_shutdown") |
| `SessionStatus::Running` | All PIDs gone | `Idle` (restorable via `session resume`) |
| `SessionStatus::Paused` | In-memory pause lost | `Idle` (restorable via `session resume`) |

`InvocationStatus::Idle` and terminal statuses (`Completed`, `Failed`, `Killed`) are
already stable — no action needed.

`session.pids` is a `Vec<u32>` of PIDs that were active in the session. After restart,
none of those PIDs are live; the list must be cleared so `resume_session` can allocate
a fresh PID without collisions.

The existing `phase3_re_adopt` adds dead agents back to the process table as `Running` —
this is wrong and will be removed from the boot sequence.

---

## Architecture references

- `docs/architecture/02-bootstrap.md` — boot phases
- `docs/architecture/14-agent-persistence.md` — invocation + session lifecycle

---

## Files to change

| # | File | Change |
|---|---|---|
| 1 | `crates/avix-core/src/bootstrap/mod.rs` | Store `invocation_store` + `session_store` on `Runtime`; pass them to crash recovery |
| 2 | `crates/avix-core/src/kernel/boot.rs` | Add `phase3_crash_recovery()`; remove `phase3_re_adopt` from boot call |

---

## Implementation order

### Step 1 — `crates/avix-core/src/bootstrap/mod.rs`

**A. Add store fields to `Runtime` struct:**

```rust
pub struct Runtime {
    // ... existing fields ...
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
}
```

Also initialise them as `None` in `bootstrap_with_root()`.

**B. In `phase2_kernel()` — store references before returning:**

After the stores are opened (the existing `let invocation_store = ...` and
`let session_store = ...` lines), clone them onto `self`:

```rust
self.invocation_store = Some(Arc::clone(&invocation_store));
self.session_store = Some(Arc::clone(&session_store));
```

**C. In `start_daemon()` — replace `phase3_re_adopt` call with crash recovery:**

Remove the existing `phase3_re_adopt(...)` block entirely.

In its place, after the `phase2_kernel()` block and *before* `phase3_services()`:

```rust
// Phase 2.5: crash recovery — fix stale Running/Paused records from prior run.
if let (Some(inv_store), Some(sess_store)) =
    (self.invocation_store.clone(), self.session_store.clone())
{
    phase3_crash_recovery(inv_store, sess_store).await?;
    self.boot_log.push(BootLogEntry {
        phase: BootPhase(2),
        message: "phase 2.5: crash recovery complete".into(),
    });
}
```

---

### Step 2 — `crates/avix-core/src/kernel/boot.rs`

**A. Add `phase3_crash_recovery` function:**

```rust
/// Phase 2.5 — fix stale invocation and session records from the previous run.
///
/// Must run before any agents are spawned and before the ATP gateway starts,
/// so no client ever observes a Running/Paused record that has no live executor.
///
/// Algorithm:
///   1. Scan all invocations; for each Running or Paused: mark Killed.
///   2. Collect the session IDs affected.
///   3. For each affected session: clear pids, then transition:
///      Running  → Idle  (allow user to resume)
///      Paused   → Idle  (in-memory pause state is lost; allow resumption)
pub async fn phase3_crash_recovery(
    invocation_store: Arc<InvocationStore>,
    session_store: Arc<PersistentSessionStore>,
) -> Result<(), AvixError> {
    info!("phase 2.5: scanning for stale records from previous run");

    let invocations = invocation_store.list_all().await?;
    let mut killed = 0u32;
    let mut affected_sessions: std::collections::HashSet<String> = Default::default();

    for inv in &invocations {
        if matches!(inv.status, InvocationStatus::Running | InvocationStatus::Paused) {
            info!(
                id = %inv.id,
                agent = %inv.agent_name,
                status = ?inv.status,
                "marking stale invocation as killed"
            );
            let _ = invocation_store
                .finalize(
                    &inv.id,
                    InvocationStatus::Killed,
                    chrono::Utc::now(),
                    inv.tokens_consumed,
                    inv.tool_calls_total,
                    Some("interrupted_at_shutdown".into()),
                )
                .await;
            killed += 1;
            affected_sessions.insert(inv.session_id.clone());
        }
    }

    // Repair affected sessions.
    let mut sessions_repaired = 0u32;
    for session_id_str in &affected_sessions {
        let session_uuid = match uuid::Uuid::parse_str(session_id_str) {
            Ok(u) => u,
            Err(_) => continue,
        };
        if let Ok(Some(mut session)) = session_store.get(&session_uuid).await {
            // Clear stale PIDs — all executors are dead after restart.
            session.pids.clear();

            // Transition non-terminal session states to Idle so the user can resume.
            match session.status {
                SessionStatus::Running | SessionStatus::Paused => {
                    session.mark_idle();
                    sessions_repaired += 1;
                }
                _ => {}
            }
            let _ = session_store.update(&session).await;
        }
    }

    info!(
        killed,
        sessions_repaired,
        "phase 2.5: crash recovery complete"
    );
    Ok(())
}
```

**B. Keep `phase3_re_adopt` function definition** but it is no longer called from
`start_daemon`. Mark it `#[allow(dead_code)]` or leave it — it may be repurposed later
for an explicit "resume all idle agents" command.

---

## Targeted tests

All tests go in `crates/avix-core/src/kernel/boot.rs` under `#[cfg(test)]`.

### Test 1 — running invocations become Killed

```
given:   InvocationStore with two records: status=Running, status=Running
when:    phase3_crash_recovery(store, session_store)
expect:  both records have status=Killed, exit_reason="interrupted_at_shutdown"
```

### Test 2 — paused invocations become Killed

```
given:   InvocationStore with one record: status=Paused
when:    phase3_crash_recovery
expect:  record has status=Killed
```

### Test 3 — idle/terminal invocations are untouched

```
given:   records with status: Idle, Completed, Failed, Killed
when:    phase3_crash_recovery
expect:  all records unchanged
```

### Test 4 — session Running → Idle

```
given:   session status=Running with pids=[42]; invocation Running in same session
when:    phase3_crash_recovery
expect:  session status=Idle, session.pids is empty
```

### Test 5 — session Paused → Idle

```
given:   session status=Paused; invocation Paused in same session
when:    phase3_crash_recovery
expect:  session status=Idle
```

### Test 6 — session with only Idle/terminal invocations is untouched

```
given:   session status=Idle; invocations all Idle/Completed
when:    phase3_crash_recovery
expect:  session unchanged (no affected_sessions entry)
```

### Test 7 — idempotent: running recovery twice is safe

```
given:   after first recovery (all Running → Killed)
when:    phase3_crash_recovery called again
expect:  no errors; counts are 0 killed, 0 sessions_repaired
```

---

## Target test coverage: 95%+ of `phase3_crash_recovery`

---

## Success criteria

1. `cargo check --package avix-core` — zero errors
2. All 7 targeted tests pass
3. `cargo clippy --package avix-core -- -D warnings` — zero warnings

---

## Post-plan architecture update

After implementation, update `docs/architecture/02-bootstrap.md`:
- Phase 2.5 description: "crash recovery — stale Running/Paused records marked Killed"
- Remove Phase 3.5 reference (no longer calls `phase3_re_adopt`)

Update `docs/architecture/14-agent-persistence.md`:
- Note that `Running`/`Paused` invocations are cleaned up at boot
- Document boot-time session repair: `Running`/`Paused` sessions → `Idle`
