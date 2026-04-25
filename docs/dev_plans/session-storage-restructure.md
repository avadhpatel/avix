# Session Storage Restructure

## Summary

Consolidate all session and invocation data per-user under `data/users/<username>/.sessions/`.
Replace global `data/sessions.redb` + `data/invocations.redb` with per-user redb files.
Replace UUID-keyed `conversation.jsonl` with PID-keyed JSONL reused for the lifetime of a PID.
Add PID invocation metadata map to `SessionRecord`.

## Architecture Spec

- `docs/architecture/14-agent-persistence.md`

## Current Layout vs New Layout

### Current

```
<avix_root>/data/
├── sessions.redb                                         ← global
├── invocations.redb                                      ← global
└── users/<username>/
    ├── sessions/<session_id>/session.yaml                ← YAML artefact
    └── agents/<agent_name>/invocations/
        ├── <uuid>.yaml                                   ← YAML summary
        └── <uuid>/conversation.jsonl                     ← keyed by UUID
```

### New

```
<avix_root>/data/users/<username>/.sessions/
├── sessions.redb                                         ← per-user
├── invocations.redb                                      ← per-user
└── <session_id>/
    ├── <pid1>.jsonl                                      ← keyed by PID, reused
    └── <pid2>.jsonl                                      ← next invocation in session
```

No YAML artefacts. No `agents/<agent_name>/invocations/` tree.

## Data Model Changes

### `InvocationRecord` — add `agent_version`

```rust
pub struct InvocationRecord {
    // existing fields...
    pub agent_version: String,   // NEW — e.g. "1.0.0"; empty string = unknown
}
```

### `SessionRecord` — add `PidInvocationMeta` + `invocation_pids`

```rust
/// Metadata stored on the session for each PID that ever ran in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PidInvocationMeta {
    pub pid: u64,
    pub invocation_id: String,
    pub agent_name: String,
    pub agent_version: String,
    pub spawned_at: DateTime<Utc>,
}

pub struct SessionRecord {
    // existing fields...
    #[serde(default)]
    pub invocation_pids: Vec<PidInvocationMeta>,   // NEW
}
```

Add method `SessionRecord::add_invocation_pid(meta: PidInvocationMeta)` (dedup by pid).

## JSONL Path Scheme

Old: `<username>/agents/<agent_name>/invocations/<uuid>/conversation.jsonl`
New: `<username>/.sessions/<session_id>/<pid>.jsonl`

The `LocalProvider` root stays at `data/users/` — only the relative path changes.

JSONL is **appended** while the PID is alive. On resume, a new PID is spawned → new JSONL file.

## Store Signatures That Change

### `InvocationStore`

```rust
// OLD
pub async fn write_conversation_structured(
    &self, id: &str, username: &str, agent_name: &str, entries: &[ConversationEntry],
) -> Result<()>

pub async fn read_conversation(
    &self, id: &str, username: &str, agent_name: &str,
) -> Result<Vec<ConversationEntry>>

// NEW
pub async fn write_conversation_structured(
    &self, pid: u64, session_id: &str, username: &str, entries: &[ConversationEntry],
) -> Result<()>

pub async fn read_conversation(
    &self, session_id: &str, pid: u64, username: &str,
) -> Result<Vec<ConversationEntry>>
```

Remove `write_yaml_artefact()` (no more YAML summaries).

### `PersistentSessionStore` (persistence.rs)

Remove `write_yaml_artefact()` and `remove_yaml_artefact()`.
Remove `with_local()` — no file artefacts written.

## New: `UserStoreRegistry`

New file: `crates/avix-core/src/kernel/user_stores.rs`

```rust
use dashmap::DashMap;
use std::{path::PathBuf, sync::Arc};

pub struct UserStores {
    pub session_store: Arc<PersistentSessionStore>,
    pub invocation_store: Arc<InvocationStore>,
}

pub struct UserStoreRegistry {
    root: PathBuf,   // <avix_root>/data/users
    stores: DashMap<String, Arc<UserStores>>,
}

impl UserStoreRegistry {
    pub fn new(root: PathBuf) -> Self { ... }

    /// Get or lazily create per-user stores.
    /// Creates `<root>/<username>/.sessions/` dir on first call.
    pub async fn for_user(&self, username: &str) -> Result<Arc<UserStores>, AvixError> { ... }

    /// Iterate all opened user stores (for crash recovery).
    pub fn all_stores(&self) -> Vec<(String, Arc<UserStores>)> { ... }

    /// Pre-open stores for all users that already have a `.sessions/` dir on disk.
    pub async fn preload_existing(&self) -> Result<(), AvixError> { ... }
}
```

`LocalProvider` for each user is rooted at `<root>/<username>/` so the JSONL relative path
is `.sessions/<session_id>/<pid>.jsonl`.

## Tracing Requirements

All new and modified code must include `tracing` instrumentation at appropriate levels.
Use `#[instrument]` on all public async fns and any fn doing I/O or redb operations.

| Level | When |
|-------|------|
| `debug!` | JSONL path resolved, entry written, store cache hit |
| `info!` | Per-user store opened/created, `.sessions/` dir created |
| `warn!` | JSONL line fails to deserialize (skip + continue), missing invocation record |
| `error!` | redb open failure, dir creation failure, write failure |

Per-step specifics:

- **Step 3** (`invocation/store.rs`): `#[instrument]` on `write_conversation_structured` and
  `read_conversation`; `debug!` when writing each JSONL batch; `warn!` on deserialize failure
- **Step 5** (`user_stores.rs`): `#[instrument]` on `for_user`, `preload_existing`;
  `info!` on first open of per-user store; `debug!` on cache hit
- **Step 8** (`kernel/proc/mod.rs`): `debug!` on `add_invocation_pid` call; `warn!` when
  registry returns error getting user store (log + degrade gracefully)
- **Step 9** (`runtime_executor.rs`): `debug!` before/after `write_conversation_structured`
- **Step 10** (`kernel/boot.rs`): `info!` per user recovered; `warn!` on user store open failure
  during recovery (skip user, continue)

## Implementation Order

### Step 1 — `invocation/record.rs`

- Add `agent_version: String` (default empty) to `InvocationRecord`
- Add to `InvocationRecord::new(...)` signature (new param)
- Update all `InvocationRecord::new(...)` call sites (grep: `InvocationRecord::new`)

Test filter: `cargo test avix_core::invocation::record`

### Step 2 — `session/record.rs`

- Add `PidInvocationMeta` struct
- Add `invocation_pids: Vec<PidInvocationMeta>` to `SessionRecord` with `#[serde(default)]`
- Add `SessionRecord::add_invocation_pid(meta: PidInvocationMeta)` (dedup by pid)
- Update `SessionRecord::new(...)` — `invocation_pids` starts empty

Test filter: `cargo test avix_core::session::record`

### Step 3 — `invocation/store.rs`

- Update `write_conversation_structured` signature: `(pid: u64, session_id: &str, username: &str, entries)`
- Update `read_conversation` signature: `(session_id: &str, pid: u64, username: &str)`
- Change JSONL relative path: `.sessions/{session_id}/{pid}.jsonl`
- Remove `write_yaml_artefact()` (no more `<uuid>.yaml` or `<uuid>/conversation.jsonl`)
- Remove `finalize()` YAML write (keep redb write only)
- Update all internal callers within this file
- Update all tests in this file to use new signatures and path assertions

Test filter: `cargo test avix_core::invocation::store`

### Step 4 — `session/persistence.rs`

- Remove `write_yaml_artefact()` and `remove_yaml_artefact()`
- Remove `with_local()` method — `PersistentSessionStore` no longer writes file artefacts
- Update `create()` and `update()` to not call YAML methods
- Update `delete()` to not call `remove_yaml_artefact`
- Update tests

Test filter: `cargo test avix_core::session::persistence`

### Step 5 — NEW `kernel/user_stores.rs`

- Implement `UserStores` struct
- Implement `UserStoreRegistry` with `for_user()`, `all_stores()`, `preload_existing()`
- `for_user()`: create `<root>/<username>/.sessions/` dir, open `sessions.redb` and
  `invocations.redb` within it, wire `LocalProvider` rooted at `<root>/<username>/`
- Export from `kernel/mod.rs`

Test filter: `cargo test avix_core::kernel::user_stores`

### Step 6 — `bootstrap/mod.rs`

- Replace:
  ```rust
  InvocationStore::open(self.root.join("data/invocations.redb"))
  PersistentSessionStore::open(self.root.join("data/sessions.redb"))
  ```
  with:
  ```rust
  let store_registry = Arc::new(UserStoreRegistry::new(self.root.join("data/users")));
  store_registry.preload_existing().await?;
  ```
- Pass `Arc<UserStoreRegistry>` to `ProcHandler` and `IpcExecutorFactory`
- Update `phase2_5_crash_recovery` to iterate `store_registry.all_stores()`
- Remove `self.invocation_store` and `self.session_store` fields → replace with `self.store_registry`

Test filter: `cargo build --package avix-core` (no isolated unit tests for bootstrap)

### Step 7 — `bootstrap/executor_factory.rs`

- Replace `Arc<InvocationStore>` + `Arc<PersistentSessionStore>` fields with `Arc<UserStoreRegistry>`
- In `create_executor()` (or equivalent): call `registry.for_user(username).await?` to get user stores
- Pass user's `invocation_store` + `session_store` to `RuntimeExecutor`

Test filter: `cargo build --package avix-core`

### Step 8 — `kernel/proc/mod.rs`

- Replace `Option<Arc<InvocationStore>>` + `Option<Arc<PersistentSessionStore>>` fields
  with `Option<Arc<UserStoreRegistry>>`
- Add `pub fn with_store_registry(mut self, r: Arc<UserStoreRegistry>) -> Self`
- In `spawn()`: call `registry.for_user(username).await?` → get user stores
- Update `create_invocation()`: populate `agent_version` from manifest if available
- After `create_invocation()`: call `session_store.update()` with new `PidInvocationMeta`
  added via `session.add_invocation_pid()`
- In `read_invocation_conversation()`: resolve `session_id` + `pid` from `InvocationRecord`
  then call `inv_store.read_conversation(session_id, pid, username)`
- Update `finalize_invocation()` — unchanged except store lookup via registry

Test filter: `cargo test avix_core::kernel::proc`

### Step 9 — `executor/runtime_executor.rs`

- Update `write_conversation_structured(id, username, agent_name, entries)` call to
  `write_conversation_structured(pid, session_id, username, entries)`
- `pid` comes from `self.pid`; `session_id` from `self.session_id` (already available)
- Update tests in this file that mock `InvocationStore`

Test filter: `cargo test avix_core::executor::runtime_executor`

### Step 10 — `kernel/boot.rs` (crash recovery)

- Update `phase3_crash_recovery` test helpers to use `UserStoreRegistry` or open stores
  at the new per-user path
- Update production crash recovery to call `registry.for_user(u).await?` per user

Test filter: `cargo test avix_core::kernel::boot`

## After All Steps: Architecture Doc Update

Update `docs/architecture/14-agent-persistence.md`:
- Disk layout section → new layout
- Remove YAML artefact references
- Add `PidInvocationMeta` to data model tables
- Update `InvocationStore::read_conversation` / `write_conversation_structured` signatures
- Update `ProcHandler::spawn()` lifecycle to show `add_invocation_pid()` call

## Testing Strategy

No workspace-wide tests. Each step uses a targeted filter. Steps 6–7 verified by
`cargo check --package avix-core` since they are wiring steps with no isolated unit tests.

Target: 95%+ coverage on touched modules via `cargo tarpaulin`.

## Invariants Preserved

- `InvocationRecord.session_id` is required — unchanged
- Crash recovery still marks `Running`/`Paused` invocations as `Killed` on boot
- JSONL append semantics preserved (entries written incrementally during `run_with_client`)
- `SessionRecord.owner_pid` is immutable — unchanged
- Kernel writes via `LocalProvider` (not VFS ACL) — unchanged
