# history-v2-gap-C: ATP Interface Extensions for Live History

## Specification Reference

- **Spec**: `docs/specs/agent-history-persistence-v2.md`
- **Phase**: v2.0 (Short-term)
- **Goal**: Add ATP handlers for interim snapshots and live invocation state

## What This Builds

New ATP operations and extends existing ones to support live (in-progress) invocation data. Integrates with gap-A (persist_interim) and gap-B (structured conversation).

## Storage Location

Data stored in user-specific folder:
```
<AVIX_ROOT>/users/<username>/.avix_data/invocations.redb
```

FS mirror at:
```
<AVIX_ROOT>/users/<username>/agents/<agent_name>/invocations/<id>/
```

## Implementation Guidance

### 1. New ATP operations

Location: `crates/avix-core/src/kernel/proc.rs`

#### proc/invocation-snapshot

Force an immediate snapshot of a running invocation.

**IPC method**: `kernel/proc/invocation-snapshot`  
**Request**: `{ "id": "<invocation-uuid>" }`  
**Response**: `{ "success": true, "record": <InvocationRecord> }`

```rust
pub async fn handle_invocation_snapshot(
    &self,
    id: &str,
) -> Result<InvocationRecord, AvixError> {
    // 1. Get the running executor for this invocation
    // 2. Call persist_interim with current state
    // 3. Return updated record
}
```

Error cases:
- Invocation not found → `AvixError::NotFound`
- Invocation already finished → `AvixError::InvalidState("invocation already finalized")`

#### proc/invocation-get (extended)

**Existing**: `kernel/proc/invocation-get`  
**New query param**: `live: Option<bool>`

**Request**: `{ "id": "<invocation-uuid>", "live": true }`  
**Response**: If `live=true` and invocation is running, returns merged state (current tokens, tool_calls, conversation excerpt). If invocation not running, behaves like `live=false`.

Implementation options:
1. **Simple**: Just return last persisted interim state (no runtime merge)
2. **Full**: Check if runtime executor exists, merge in-flight state

For v2.0, use option 1 (return last snapshot) — full merge can be v2.1.

#### proc/invocation-list (extended)

**Existing**: `kernel/proc/invocation-list`  
**New query param**: `live: Option<bool>`

**Request**: `{ "username": "alice", "live": true }`  
**Response**: Includes currently-running invocations when `live=true`.

### 2. IPC handlers

Location: `crates/avix-core/src/kernel/proc.rs`

```rust
impl KernelProcHandler {
    /// Force an immediate snapshot of a running invocation.
    pub async fn handle_invocation_snapshot(
        &self,
        id: &str,
    ) -> Result<InvocationRecord, AvixError> {
        // Get invocation record
        let record = self
            .invocation_store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {}", id)))?;
        
        // Check it's still running
        if !matches!(record.status, InvocationStatus::Running | InvocationStatus::Idle) {
            return Err(AvixError::InvalidState(
                "cannot snapshot finalized invocation".into(),
            ));
        }
        
        // Get current runtime state from executor
        // This requires storing invocation_id -> executor reference
        // For v2.0: just call persist_interim without merging runtime state
        self.invocation_store
            .persist_interim(id, &[], record.tokens_consumed, record.tool_calls_total)
            .await?;
        
        self.invocation_store
            .get(id)
            .await?
            .ok_or_else(|| AvixError::NotFound(format!("invocation {}", id)))
    }
    
    /// Get invocation details, optionally with live state.
    pub async fn handle_invocation_get(
        &self,
        id: &str,
        live: bool,
    ) -> Result<Option<InvocationRecord>, AvixError> {
        let record = self.invocation_store.get(id).await?;
        
        if live && record.is_some() {
            let rec = record.as_ref().unwrap();
            if matches!(rec.status, InvocationStatus::Running | InvocationStatus::Idle) {
                // Could merge runtime state here in v2.1
                // For v2.0: just return persisted state
            }
        }
        
        Ok(record)
    }
    
    /// List invocations, optionally including live ones.
    pub async fn handle_invocation_list(
        &self,
        username: &str,
        agent_name: Option<&str>,
        live: Option<bool>,
    ) -> Result<Vec<InvocationRecord>, AvixError> {
        let records = match agent_name {
            Some(name) => self.invocation_store.list_for_agent(username, name).await,
            None => self.invocation_store.list_for_user(username).await,
        }?;
        
        if live == Some(true) {
            // Include running invocations
            // v2.0: all Running status records are already in the store
            Ok(records)
        } else {
            // Filter out Running (only show finalized)
            Ok(records
                .into_iter()
                .filter(|r| !matches!(r.status, InvocationStatus::Running | InvocationStatus::Idle))
                .collect())
        }
    }
}
```

### 3. Gateway forwarding

Location: `crates/avix-core/src/gateway/mod.rs` or similar

Add the new ATP operation to the forward table:

```rust
// In the ATP -> IPC routing table
"proc/invocation-snapshot" => "kernel/proc/invocation-snapshot",
```

### 4. Client command helpers

Location: `crates/avix-client-core/src/commands.rs`

```rust
pub async fn snapshot_invocation(dispatcher, invocation_id) -> Result<InvocationRecord>
pub async fn get_invocation_live(dispatcher, invocation_id) -> Result<Option<InvocationRecord>>
pub async fn list_invocations_live(dispatcher, username, agent_name) -> Result<Vec<InvocationRecord>>
```

### 5. CLI extensions

Location: `crates/avix-cli/src/commands/agent.rs`

```bash
# New commands
avix agent snapshot <invocation-id>    # Force snapshot
avix agent history --live              # Include running invocations

# Extended
avix agent show <id> --live            # Show live state
```

### 6. Test helpers for runtime state merge (future)

For v2.1, we need to track running invocations in a way that's queryable:

```rust
// In KernelProcHandler or ProcHandler
pub async fn get_running_invocation(&self, id: &str) -> Option<InvocationRecord> {
    // Check active_invocations map
    // Merge with persisted record
}
```

This requires the executor to report its invocation_id to the kernel at spawn.

## TDD Tests

```rust
// In crates/avix-core/src/kernel/proc.rs integration tests

// T-PROC-01
#[tokio::test]
async fn invocation_snapshot_returns_updated_record() {
    // Create invocation
    // Call snapshot
    // Verify tokens/tool_calls updated
}

// T-PROC-02  
#[tokio::test]
async fn invocation_snapshot_fails_for_finalized() {
    // Create and finalize invocation
    // Call snapshot → error
}

// T-PROC-03
#[tokio::test]
async fn invocation_get_live_includes_running() {
    // Start agent (Running status)
    // Call get with live=true
    // Verify returns the Running record
}

// T-PROC-04
#[tokio::test]
async fn invocation_list_filters_live() {
    // Create one completed, one running
    // Call list with live=false → only completed
    // Call list with live=true → both
}
```

## Success Criteria

- [ ] `proc/invocation-snapshot` ATP call implemented
- [ ] `proc/invocation-get` supports `live` query param
- [ ] `proc/invocation-list` supports `live` query param
- [ ] Gateway forwards new operations
- [ ] CLI commands work: `avix agent snapshot <id>`, `avix agent history --live`
- [ ] All tests pass: `cargo test --workspace`