# history-v2-gap-A: Interim Snapshots (Live Persistence)

## Specification Reference

- **Spec**: `docs/specs/agent-history-persistence-v2.md`
- **Phase**: v2.0 (Short-term)
- **Goal**: Make history useful *during* a run, not just after

## What This Builds

Adds `persist_interim()` capability to `InvocationStore` that writes current state to redb + FS mirror without finalizing the record. `RuntimeExecutor` calls this hook after every LLM turn or N tool calls.

## Implementation Guidance

### 1. Add `persist_interim` to `InvocationStore`

Location: `crates/avix-core/src/invocation/store.rs`

```rust
/// Write interim snapshot of a running invocation.
/// Unlike `finalize()`, this does NOT set ended_at or change status.
pub async fn persist_interim(
    &self,
    id: &str,
    conversation: &[(String, String)],  // role, content pairs
    tokens_consumed: u64,
    tool_calls_total: u32,
) -> Result<(), AvixError>
```

- Reads existing record by id
- Updates `tokens_consumed` and `tool_calls_total` (these may change during run)
- Does NOT modify `status`, `ended_at`, or `exit_reason`
- Writes updated record to redb + YAML artefact
- Appends conversation to `conversation.jsonl` (or updates if already written)

### Storage Location

All user-specific data (including redb) is stored in:
```
<AVIX_ROOT>/users/<username>/.avix_data/
├── invocations.redb    ← primary store for InvocationRecord
├── sessions.redb       ← (future) SessionStore
└── history.redb       ← (future) HistoryStore for MessageRecord/PartRecord
```

The LocalProvider FS mirror continues to write to:
```
<AVIX_ROOT>/users/<username>/agents/<agent_name>/invocations/<id>/
├── <id>.yaml           ← summary
└── conversation.jsonl  ← conversation
```

### 2. New redb table for snapshots (optional v2.0)

```rust
const SNAPSHOT_TABLE: TableDefinition<&str, &str> = TableDefinition::new("invocation_snapshots");
// key: "{invocation_id}:{sequence}"
// value: JSON snapshot of conversation state at that point
```

For v2.0, we can skip this and just update the main record. Snapshot sequencing can be added in v2.1.

### 3. RuntimeExecutor hook

Location: `crates/avix-core/src/executor/runtime_executor.rs`

Add configuration:
```rust
// In RuntimeExecutor config or spawn params
snapshot_interval: Option<u32>,  // N tool calls; None = disabled
```

In the main loop, after each LLM turn completes:
```rust
if let Some(interval) = self.snapshot_interval {
    self.tool_call_count += 1;
    if self.tool_call_count >= interval {
        self.invocation_store.persist_interim(
            &self.invocation_id,
            &self.conversation_history,
            self.tokens_consumed,
            self.tool_calls_total,
        ).await;
        self.tool_call_count = 0;
    }
}
```

Also call on SIGSAVE (user-requested snapshot).

### 4. Environment variable for config

```rust
let snapshot_interval = std::env::var("AVIX_HISTORY_SNAPSHOT_INTERVAL")
    .ok()
    .and_then(|v| v.parse().ok());
```

### 5. ATP handler for force snapshot

Location: `crates/avix-core/src/kernel/proc.rs`

```rust
pub async fn handle_invocation_snapshot(
    &self,
    id: &str,
) -> Result<InvocationRecord, AvixError>
```

- Calls `persist_interim` with current runtime state
- Returns updated record

### 6. Extended `proc/invocation-get` with `--live` flag

```rust
pub async fn handle_invocation_get(
    &self,
    id: &str,
    live: bool,  // NEW: if true, reads from runtime state instead of finalized record
) -> Result<Option<InvocationRecord>, AvixError>
```

For live=true, we need to either:
- Check if invocation is still running and merge with runtime state
- Or just return the last interim snapshot

### 7. Extended `proc/invocation-list` with `live` filter

```rust
pub async fn handle_invocation_list(
    &self,
    username: &str,
    agent_name: Option<&str>,
    live: Option<bool>,  // NEW: if Some(true), includes running invocations
) -> Result<Vec<InvocationRecord>, AvixError>
```

## TDD Tests

```rust
// In crates/avix-core/src/invocation/store.rs tests

// T-INV-09
#[tokio::test]
async fn persist_interim_updates_tokens_and_tool_calls() {
    let store = open_store().await;
    let rec = make_record("inv-09", "alice", "researcher");
    store.create(&rec).await.unwrap();
    
    // Initial state
    let loaded = store.get("inv-09").await.unwrap().unwrap();
    assert_eq!(loaded.tokens_consumed, 0);
    assert_eq!(loaded.tool_calls_total, 0);
    
    // Interim update
    store.persist_interim(
        "inv-09",
        &[],
        1500,
        5,
    ).await.unwrap();
    
    let loaded = store.get("inv-09").await.unwrap().unwrap();
    assert_eq!(loaded.tokens_consumed, 1500);
    assert_eq!(loaded.tool_calls_total, 5);
    assert_eq!(loaded.status, InvocationStatus::Running);  // unchanged
    assert!(loaded.ended_at.is_none());  // unchanged
}

// T-INV-10
#[tokio::test]
async fn persist_interim_writes_conversation_partial() {
    // Write partial conversation during run
    // Verify it's readable before finalize
}

// T-INV-11
#[tokio::test]
async fn persist_interim_unknown_id_is_idempotent() {
    // Should succeed silently if id doesn't exist
}
```

## Success Criteria

- [x] `InvocationStore::persist_interim` writes to redb without changing status/ended_at
- [x] RuntimeExecutor calls persist_interim after N tool calls (configurable via env var)
- [x] SIGSAVE triggers immediate snapshot
- [ ] `proc/invocation-snapshot <id>` ATP call works (deferred to gap-C)
- [ ] `proc/invocation-get <id> --live` returns latest state (deferred to gap-C)
- [x] Existing runs (without snapshot_interval) continue to work unchanged
- [x] All tests pass: `cargo test --workspace`