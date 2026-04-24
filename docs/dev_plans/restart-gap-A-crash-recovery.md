# restart-gap-A — Boot-time Crash Recovery with Agent Restoration

> Goal: On every `avix start`, repair stale persistence records and re-spawn live
> executors for any agent that was Running, Paused, or Idle at crash/shutdown time,
> so users can immediately send messages to those agents without manual resumption.

---

## Problem

After a crash or clean shutdown, `InvocationStore` (redb) and `PersistentSessionStore`
(redb) contain records whose in-memory executors are gone.

| Record status | After restart: invocation | After restart: session |
|---|---|---|
| `Running` | Restore live executor → Idle/Waiting | Clear pids → Idle |
| `Paused` | Restore live executor → Idle/Waiting | Clear pids → Idle |
| `Idle` | Restore live executor → Idle/Waiting | (already Idle, but clear stale pids) |
| `Completed` / `Failed` / `Killed` | Untouched (terminal) | Untouched |

The restored executor:
- Loads conversation history from the persisted JSONL (keyed by `InvocationRecord.pid`)
- Gets a **new PID** (for signal-channel registration and process-table entry)
- Re-mints a capability token (using agent manifest + master key)
- Goes directly to `idle()` then `wait_for_next_goal()` — no LLM call until user sends next message
- Accepts SIGSTART to receive the next user message

`session.pids` is cleared on all sessions whose agents are being restored. `session.status`
is set to `Idle` for any Running/Paused session. The session remains Idle until the first
SIGSTART triggers a real turn.

---

## Architecture references

- `docs/architecture/02-bootstrap.md` — boot phases
- `docs/architecture/14-agent-persistence.md` — invocation + session lifecycle

---

## Files to change

| # | File | Change |
|---|---|---|
| 1 | `crates/avix-core/src/executor/spawn.rs` | Add `restore_from_pid: Option<u64>` to `SpawnParams` |
| 2 | `crates/avix-core/src/bootstrap/executor_factory.rs` | Restore mode in `launch()`: load JSONL, skip initial turn, go to idle+wait loop |
| 3 | `crates/avix-core/src/kernel/proc/agent.rs` | Make `resolve_granted_tools` pub; add `restore_from_invocation()` |
| 4 | `crates/avix-core/src/kernel/proc/mod.rs` | Add `restore_interrupted_agents()` to `ProcHandler` |
| 5 | `crates/avix-core/src/kernel/boot.rs` | `phase3_crash_recovery` only repairs sessions; update tests |
| 6 | `crates/avix-core/src/bootstrap/mod.rs` | Phase 3.5: call `proc_handler.restore_interrupted_agents()` after services |

---

## Implementation

### File 1 — `crates/avix-core/src/executor/spawn.rs`

Add `restore_from_pid: Option<u64>` to `SpawnParams`.
When `Some(old_pid)`, `IpcExecutorFactory::launch()` enters restore mode:
loads conversation history from `<username>/.sessions/<session_id>/<old_pid>.jsonl`.

### File 2 — `crates/avix-core/src/bootstrap/executor_factory.rs`

In `IpcExecutorFactory::launch()`, after wiring infrastructure, check `params.restore_from_pid`:

**Normal path** (existing): emit `agent_spawned`, run `run_with_client(goal)`, idle, loop.

**Restore path** (`restore_from_pid = Some(old_pid)`):
1. Load conversation history: `invocation_store.read_conversation(session_id, old_pid, username).await`
2. Inject into `executor.memory.conversation_history`
3. Register signal channel
4. Call `executor.idle()` (persists Idle status; writes JSONL back to old pid path)
5. Set process table → Waiting
6. Emit `agent_status(atp_session_id="", pid, "waiting")`
7. Enter `wait_for_next_goal()` loop — identical to normal post-turn loop

### File 3 — `crates/avix-core/src/kernel/proc/agent.rs`

- Change `resolve_granted_tools` from `async fn` to `pub async fn`.
- Add `pub async fn restore_from_invocation(inv: &InvocationRecord) -> Option<u64>`:
  1. Allocate new `pid = Pid::generate().as_u64()`
  2. Resolve granted tools (re-minted token, 3600s TTL)
  3. Insert `ProcessEntry` into process table with `ProcessStatus::Waiting`
  4. Add pid to session in session_store
  5. Register in `active_sessions` and `active_invocations`
  6. Call `executor_factory.launch(SpawnParams { restore_from_pid: Some(inv.pid), ... })`
  7. Store abort handle in `task_handles`
  8. Return `Some(pid)`

### File 4 — `crates/avix-core/src/kernel/proc/mod.rs`

Add `pub async fn restore_interrupted_agents(&self, invocation_store: Arc<InvocationStore>)`:
1. `let invocations = invocation_store.list_all().await?`
2. Filter: `Running | Paused | Idle` status
3. For each: call `self.agent_manager.restore_from_invocation(&inv).await`
4. Log counts

### File 5 — `crates/avix-core/src/kernel/boot.rs`

Update `phase3_crash_recovery`:
- Scan for sessions whose agents were Running/Paused
- Clear pids, set session status → Idle
- **Do NOT finalize/kill any invocations** (they will be restored in phase 3.5)

Update tests:
- Remove assertions that Running/Paused → Killed
- Assert sessions are Idle + pids cleared
- Assert invocation statuses are unchanged

### File 6 — `crates/avix-core/src/bootstrap/mod.rs`

In `start_daemon()`, after `phase3_services()`:

```rust
// Phase 3.5: restore interrupted agents from previous run.
if let (Some(ph), Some(inv_store)) =
    (self.proc_handler.as_ref(), self.invocation_store.as_ref())
{
    ph.restore_interrupted_agents(Arc::clone(inv_store)).await;
    self.boot_log.push(BootLogEntry {
        phase: BootPhase(3),
        message: "phase 3.5: interrupted agents restored".into(),
    });
}
```

---

## Boot sequence

```
Phase 2:   kernel.agent spawned; stores opened
Phase 2.5: phase3_crash_recovery — repair sessions (clear pids; Running/Paused → Idle)
Phase 3:   services spawned; real ToolRegistry injected into executor factory
Phase 3.5: restore_interrupted_agents — spawn live executors for all non-terminal invocations
Phase 4:   ATP gateway — clients can now connect and SIGSTART restored agents
```

---

## Key invariant: JSONL path consistency

`persist_interim_structured(invocation_id, ...)` uses `InvocationRecord.pid` (old pid)
to determine JSONL path. Restored executor gets new PID for signal routing but the same
`invocation_id` in params. So:
- Read: `read_conversation(session_id, old_pid, username)` → old JSONL
- Write (via `idle()`): `persist_interim_structured(invocation_id, ...)` → same old JSONL via record.pid

No inconsistency. No data loss.

---

## Success criteria

1. `cargo check --package avix-core` — zero errors
2. `cargo clippy --package avix-core -- -D warnings` — zero warnings
3. After `avix start` following a crash: all previously Running/Paused/Idle agents appear
   in process table as Waiting and respond to SIGSTART

---

## Post-plan architecture update

After implementation, update `docs/architecture/02-bootstrap.md`:
- Phase 2.5: "session repair — stale Running/Paused sessions cleared; invocations untouched"
- Phase 3.5: "agent restoration — live executors spawned for all non-terminal invocations"
