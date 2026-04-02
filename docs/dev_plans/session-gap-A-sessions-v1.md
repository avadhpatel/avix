# Session Management — Dev Plan (Phase 1: Sessions v1.0)

> **Status:** Pending  
> **Priority:** High — enables multi-turn agent workflows, completes history v2 roadmap  
> **Depends on:** `docs/architecture/14-agent-persistence.md` (current invocation model)  
> **Blocks:** Phase 2 (full hierarchy, messages, resume), Phase 3 (collaboration)  
> **Affects:** `crates/avix-core/src/invocation/`, `crates/avix-core/src/session/`, `crates/avix-core/src/kernel/proc.rs`, `crates/avix-core/src/gateway/handlers/proc.rs`, `crates/avix-cli/`

---

## Overview

Implement first-class **Session** abstraction as a persistent container for one or more agent Invocations. This enables multi-turn agent workflows where an agent can pause (Idle), be resumed later, and maintain full conversation history.

Phase 1 delivers:
1. `Idle` status for both Sessions and Invocations
2. New `SessionRecord` with persistence (redb + VFS mirror)
3. Updated `InvocationRecord` with required `session_id`
4. RuntimeExecutor spawn/shutdown logic for session management
5. ATP handlers: `proc/session/create`, `proc/session/list`, `proc/session/get`, `proc/session/resume`
6. Backward compatibility: existing single-invocation agents continue to work

---

## What to Implement

### Task 1: Add `Idle` to `InvocationStatus` enum

**File:** `crates/avix-core/src/invocation/record.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InvocationStatus {
    #[default]
    Running,
    Idle,      // NEW — waiting for input
    Completed,
    Failed,
    Killed,
}
```

**Test:** Existing tests in `record.rs` should still pass; add new test for `Idle` serialization.

---

### Task 2: Create new `SessionRecord` type

**New file:** `crates/avix-core/src/session/record.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── SessionStatus ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    #[default]
    Running,
    Idle,       // NEW — all invocations Idle/Completed, waiting for input
    Completed,
    Failed,
    Archived,
}

// ── SessionRecord ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,           // for forking/branching sessions
    pub project_id: Option<String>,        // future workspace / folder support
    pub title: String,                     // auto or user-provided
    pub goal: String,
    pub username: String,
    pub spawned_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub status: SessionStatus,
    pub summary: Option<String>,           // high-level summary (updated on Idle)
    pub tokens_total: u64,
    
    // Multi-agent tracking
    pub origin_agent: String,               // agent_name that started the session (first spawn)
    pub primary_agent: String,              // agent_name currently in control / primary
    pub participants: Vec<String>,         // all agent_names involved (origin + primary + others)
}

impl SessionRecord {
    pub fn new(
        id: Uuid,
        username: String,
        origin_agent: String,
        title: String,
        goal: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            parent_id: None,
            project_id: None,
            title,
            goal,
            username,
            spawned_at: now,
            last_updated: now,
            status: SessionStatus::Running,
            summary: None,
            tokens_total: 0,
            origin_agent: origin_agent.clone(),
            primary_agent: origin_agent,
            participants: vec![],
        }
    }

    pub fn mark_idle(&mut self) {
        self.status = SessionStatus::Idle;
        self.last_updated = Utc::now();
    }

    pub fn mark_running(&mut self) {
        self.status = SessionStatus::Running;
        self.last_updated = Utc::now();
    }

    /// Add a new agent to the session (e.g., when spawning a sub-agent).
    /// Updates primary_agent if this agent is now the one in focus.
    pub fn add_participant(&mut self, agent_name: &str, make_primary: bool) {
        if !self.participants.contains(&agent_name.to_string()) {
            self.participants.push(agent_name.to_string());
        }
        if make_primary {
            self.primary_agent = agent_name.to_string();
        }
        self.last_updated = Utc::now();
    }

    /// Promote a participant to be the primary agent (e.g., after sub-agent completes).
    pub fn set_primary(&mut self, agent_name: &str) {
        if self.participants.contains(&agent_name.to_string()) || agent_name == &self.origin_agent {
            self.primary_agent = agent_name.to_string();
            self.last_updated = Utc::now();
        }
    }

    pub fn add_tokens(&mut self, tokens: u64) {
        self.tokens_total += tokens;
        self.last_updated = Utc::now();
    }
}
```

**Test file** (in `record.rs` under `#[cfg(test)]`):
- `session_record_new_defaults` — status = Running, participants empty
- `mark_idle_sets_status_and_timestamp`
- `mark_running_sets_status_and_timestamp`
- `add_participant_avoids_duplicates`
- `session_status_serialises_lowercase` — "idle", "running", "completed", "failed", "archived"
- `roundtrip_json` — serialize → deserialize preserves all fields

---

### Task 3: Create `SessionStore` for persistence

**New file:** `crates/avix-core/src/session/persistence.rs`

Build on the existing `InvocationStore` pattern (redb + LocalProvider mirror).

```rust
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use redb::{Database, TableDefinition};
use tokio::sync::Mutex;

use super::record::{SessionRecord, SessionStatus};
use crate::error::AvixError;
use crate::memfs::local_provider::LocalProvider;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("sessions");

pub struct SessionStore {
    db: Arc<Mutex<Database>>,
    local: Option<Arc<LocalProvider>>,
}

impl SessionStore {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, AvixError> {
        let path = path.into();
        let db = Database::create(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            write_txn.open_table(TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(Self { db: Arc::new(Mutex::new(db)), local: None })
    }

    pub fn with_local(mut self, provider: LocalProvider) -> Self {
        self.local = Some(Arc::new(provider));
        self
    }

    pub async fn create(&self, record: &SessionRecord) -> Result<(), AvixError> {
        let json = serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn.open_table(TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table.insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(record).await;
        Ok(())
    }

    pub async fn get(&self, id: &Uuid) -> Result<Option<SessionRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn.open_table(TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        match table.get(id.to_string().as_str()).map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            Some(v) => {
                let record: SessionRecord = serde_json::from_str(v.value())
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    pub async fn update(&self, record: &SessionRecord) -> Result<(), AvixError> {
        let json = serde_json::to_string(record).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let db = self.db.lock().await;
        let write_txn = db.begin_write().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        {
            let mut table = write_txn.open_table(TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            table.insert(record.id.to_string().as_str(), json.as_str())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        }
        write_txn.commit().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.write_yaml_artefact(record).await;
        Ok(())
    }

    pub async fn list_for_user(&self, username: &str) -> Result<Vec<SessionRecord>, AvixError> {
        let db = self.db.lock().await;
        let read_txn = db.begin_read().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let table = read_txn.open_table(TABLE).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        use redb::ReadableTable;
        let mut entries = Vec::new();
        for item in table.iter().map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            let (_, v) = item.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let record: SessionRecord = serde_json::from_str(v.value())
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            if record.username == username {
                entries.push(record);
            }
        }
        Ok(entries)
    }

    // VFS artefact at /users/<username>/sessions/<id>/session.yaml
    async fn write_yaml_artefact(&self, record: &SessionRecord) {
        let provider = match &self.local {
            Some(p) => p,
            None => return,
        };
        if record.username.is_empty() {
            return;
        }
        let yaml = match serde_yaml::to_string(record) {
            Ok(y) => y,
            Err(_) => return,
        };
        let rel = format!("{}/sessions/{}/session.yaml", record.username, record.id);
        let _ = provider.write(&rel, yaml.into_bytes()).await;
    }
}
```

Wire into `session/mod.rs`:

```rust
pub mod entry;
pub mod record;       // NEW
pub mod store;
pub mod persistence; // NEW

pub use entry::{AgentRef, AgentRole, QuotaSnapshot, SessionEntry, SessionState};
pub use record::{SessionRecord, SessionStatus};  // NEW
pub use store::SessionStore;
pub use persistence::SessionStore as PersistentSessionStore;  // NEW (rename to avoid conflict)
```

---

### Task 4: Wire SessionStore into ProcHandler

**File:** `crates/avix-core/src/kernel/proc.rs`

Add fields:

```rust
pub struct ProcHandler {
    // ... existing fields ...
    /// Persistent store for session records.
    session_store: Option<Arc<PersistentSessionStore>>,
    // ... existing fields ...
}
```

Add builder methods:

```rust
impl ProcHandler {
    pub fn with_session_store(mut self, store: Arc<PersistentSessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }
}
```

**Updated spawn logic (with optional session_id from parent agent):**

```rust
pub async fn spawn(
    &self,
    name: &str,                      // agent_name being spawned
    goal: &str,
    session_id: &str,                 // OPTIONAL - can be empty
    caller_identity: &str,
    parent_agent_name: Option<&str>,  // NEW: name of agent that requested this spawn
) -> Result<u32, AvixError> {
    // Resolve session: attach to existing OR create new
    let effective_session_id = if session_id.is_empty() {
        // No session_id provided - create a new session
        // This is either a fresh session OR a parent spawning its first sub-agent
        if let Some(store) = &self.session_store {
            let origin = parent_agent_name.unwrap_or(name);
            let title = name.to_string();
            let record = SessionRecord::new(
                Uuid::new_v4(),
                caller_identity.to_string(),
                origin.to_string(),
                title,
                goal.to_string(),
            );
            store.create(&record).await.ok();
            record.id.to_string()
        } else {
            Uuid::new_v4().to_string()
        }
    } else {
        // Session ID provided - attach to existing session
        // This is a parent agent spawning a sub-agent into its session
        if let Some(store) = &self.session_store {
            if let Ok(Some(mut session)) = store.get(&Uuid::parse_str(session_id)?).await {
                // Add this agent as participant, make it primary if it's a new sub-agent
                session.add_participant(name, true);
                store.update(&session).await.ok();
            }
        }
        session_id.to_string()
    };
    
    // Continue with existing spawn logic using effective_session_id
}
```

**IPC handlers for session operations:**

```rust
"kernel/proc/session/create" => {
    // Parse body: { title, goal, origin_agent?, parent_id? }
    // Create SessionRecord with new UUID
    // If session_store present: store.create(&record).await
    // Return: { session_id: UUID }
}

"kernel/proc/session/list" => {
    // Parse body: { username }
    // Return: Vec<SessionRecord> (filter by username)
}

"kernel/proc/session/get" => {
    // Parse body: { id: UUID }
    // Return: Option<SessionRecord>
}

"kernel/proc/session/resume" => {
    // Parse body: { session_id: UUID, input? }
    // Load session, verify status is Idle or Running
    // Spawn new invocation attached to same session_id
    // Return: { invocation_id, pid }
}
```

---

### Task 5: Update RuntimeExecutor spawn/shutdown logic

**File:** `crates/avix-core/src/executor/runtime_executor.rs`

Modify `shutdown_with_status`:

```rust
pub async fn shutdown_with_status(&self, status: InvocationStatus, exit_reason: Option<String>) {
    // Existing logic...
    
    // NEW: If agent signals "waiting_for_input", transition to Idle instead of finalizing
    if exit_reason.as_deref() == Some("waiting_for_input") {
        if let Some(store) = &self.invocation_store {
            store.update_status(&self.invocation_id, InvocationStatus::Idle).await;
        }
        // Update session status to Idle if all invocations are Idle/Completed
        if let Some(session_store) = &self.session_store {
            let mut session = session_store.get(&self.session_id).await?.unwrap();
            session.mark_idle();
            session_store.update(&session).await;
        }
        return;  // Don't finalize - agent is Idle, not terminated
    }
    
    // Existing finalize logic...
}
```

Add method to `InvocationStore`:

```rust
pub async fn update_status(&self, id: &str, status: InvocationStatus) -> Result<(), AvixError> {
    let mut record = self.get(id).await?.ok_or_else(|| AvixError::NotFound(id.to_string()))?;
    record.status = status;
    // ... update in redb ...
}
```

---

### Task 6: Add ATP event kinds for session tracking

**File:** `crates/avix-core/src/gateway/atp/types.rs`

Add new event kinds:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AtpEventKind {
    // ... existing ...
    
    /// Agent attached to an Avix Session (spawned into existing session)
    #[serde(rename = "session.agent.attached")]
    SessionAgentAttached,
    
    /// Agent detached / completed in a session  
    #[serde(rename = "session.agent.detached")]
    SessionAgentDetached,
    
    /// Session status changed (Running → Idle → Completed)
    #[serde(rename = "session.status")]
    SessionStatusChanged,
}
```

**File:** Update `event_bus.rs` event_scope function to include new events.

---

### Task 7: Add ATP proc handlers for sessions

**File:** `crates/avix-core/src/gateway/handlers/proc.rs`

Update handler:

```rust
pub async fn handle(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    let id = cmd.cmd.id.clone();
    let op = cmd.cmd.op.as_str();

    match op {
        "spawn" | "kill" | "list" | "stat" | "pause" | "resume" | "wait" | "setcap"
        | "list-installed" | "invocation-list" | "invocation-get"
        | "session-create" | "session-list" | "session-get" | "session-resume" => {  // NEW ops
            let ipc_method = format!("kernel/proc/{}", op.replace("session-", "session/"));
            ipc_forward(&id, &ipc_method, cmd.cmd.body, ctx.ipc.as_ref()).await
        }
        op => unknown_op(id, op),
    }
}
```

Note: ATP uses `session-list` (kebab), IPC uses `kernel/proc/session/list` (path).

---

### Task 8: Update CLI for session commands

**File:** `crates/avix-cli/src/commands/agent.rs` or new `session.rs`

```bash
# Create a new session
avix session create --title "Fix bug 123" --goal "Debug and fix the login crash"

# List sessions
avix session list [--username alice] [--status idle|running|completed]

# Get session details
avix session show <session-id>

# Resume a session (spawn new invocation in existing session)
avix session resume <session-id> --input "Continue from where we left off"
```

Add to CLI enum in `main.rs` or `lib.rs`.

---

### Task 9: Backward compatibility and testing

Existing code that calls `proc/spawn` without `session_id` continues to work:
- Old: `proc/spawn { name, goal }` → auto-creates a new Session with `origin_agent = name`
- New: Optional `session_id` lets agents spawn sub-agents into the parent's session

**Test scenarios:**
1. **Direct spawn (no session_id):** Creates new Session with agent as origin/primary
2. **Sub-agent spawn (with session_id):** Attaches to existing session, adds to participants
3. **Resume session:** Creates new invocation in existing session
4. **Kill sub-agent:** Updates primary_agent back to origin_agent

---

## TDD Approach

### Multi-agent session tests

In addition to the tests above, add:

5. **`session/record.rs`**:
   - `origin_agent_set_on_creation` — first agent becomes origin_agent
   - `add_participant_with_make_primary` — new agent becomes primary
   - `set_primary_swaps_primary_agent` — primary changes without adding duplicate
   - `origin_agent_never_removed` — origin_agent persists even after primary changes

6. **`kernel/proc.rs`** (integration):
   - `spawn_without_session_id_creates_new_session_with_origin`
   - `spawn_with_session_id_attaches_to_existing_session`
   - `sub_agent_completes_and_primary_returns_to_origin`

1. **`invocation/record.rs`**: Test `InvocationStatus::Idle` serialization
2. **`session/record.rs`**: All tests from Task 2
3. **`session/persistence.rs`**: 
   - `create_saves_record_to_redb`
   - `get_returns_record_by_id`
   - `update_modifies_existing_record`
   - `list_for_user_filters_by_username`
   - `create_idempotent_on_duplicate_key` (should fail, then handle)
4. **`kernel/proc.rs`** (integration):
   - `session_store_is_none_by_default`
   - `with_session_store_configures_store`
5. **`runtime_executor.rs`**:
   - `shutdown_with_status_waiting_for_input_marks_idle`
   - `shutdown_with_status_normal_finalizes`

### Success Criteria

- [ ] `InvocationStatus::Idle` serializes as `"idle"` (lowercase)
- [ ] `SessionRecord` has all fields: `origin_agent`, `primary_agent`, `participants`
- [ ] Session creation sets origin_agent = first agent, primary_agent = origin
- [ ] Adding participant with `make_primary=true` updates both `participants` and `primary_agent`
- [ ] `SessionStore::create` persists to redb + writes YAML artefact
- [ ] `SessionStore::get` retrieves by UUID
- [ ] `SessionStore::list_for_user` filters by username
- [ ] `ProcHandler::spawn` accepts optional `session_id`, auto-creates if empty
- [ ] `ProcHandler::spawn` with `session_id` attaches to existing session, adds participant
- [ ] `RuntimeExecutor::shutdown_with_status` handles "waiting_for_input" → Idle
- [ ] Sub-agent completion promotes `origin_agent` back to `primary_agent`
- [ ] ATP events include `session_id` (Avix Session) for sidebar grouping
- [ ] ATP `session-create`, `session-list`, `session-get`, `session-resume` forward correctly
- [ ] CLI `avix session *` commands registered
- [ ] Existing `proc/invocation-*` calls unchanged (backward compat)
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
- [ ] `cargo fmt --check` — zero formatting diff

---

## Detailed Implementation Guidance

### Crates/Files to Modify

| File | Change |
|------|--------|
| `invocation/record.rs` | Add `Idle` variant to `InvocationStatus` |
| `session/record.rs` | New — `SessionRecord`, `SessionStatus` |
| `session/persistence.rs` | New — `SessionStore` for redb + VFS |
| `session/mod.rs` | Export new types |
| `kernel/proc.rs` | Add `session_store` field, update `spawn` for auto-session |
| `kernel/ipc_server.rs` | Add handlers for `kernel/proc/session/*` |
| `gateway/handlers/proc.rs` | Add session ops to match list |
| `gateway/atp/types.rs` | Add new event kinds: `SessionAgentAttached`, `SessionAgentDetached`, `SessionStatusChanged` |
| `gateway/event_bus.rs` | Add session_id to agent event payloads |
| `executor/runtime_executor.rs` | Handle `waiting_for_input` → Idle transition |
| `invocation/store.rs` | Add `update_status` method |
| `cli/commands/session.rs` | New — CLI session commands |

### Key Functions to Add/Modify

1. **`SessionRecord::new`** — constructor with required fields
2. **`SessionRecord::{mark_idle, mark_running}`** — status transitions
3. **`SessionStore::{create, get, update, list_for_user}`** — persistence ops
4. **`ProcHandler::spawn`** — auto-create session if empty, add participant if session_id provided
5. **`RuntimeExecutor::shutdown_with_status`** — handle Idle transition, update primary_agent
6. **IPC dispatch** — add `kernel/proc/session/create`, `session/list`, `session/get`, `session/resume`
7. **ATP event emission** — emit `AgentSpawned`, `SessionAgentAttached`, `SessionStatusChanged` with session_id

### Tracing Points

Add `tracing::info!` for:
- Session creation (`session_id`, `username`, `origin_agent`)
- Session resume (`session_id`, `new_invocation_id`)
- Agent attached to session (`session_id`, `agent_name`, `is_sub_agent`)
- Idle transition (`invocation_id`, `session_id`)
- Primary agent promotion (`session_id`, `old_primary`, `new_primary`)

### ATP Events for Sidebar Grouping

ATP events already include `sessionId` field (the ATP session, not Avix Session). To support sidebar grouping by Avix Session:

**Add new event kinds:**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AtpEventKind {
    // ... existing ...
    
    /// Agent attached to an Avix Session (spawned into existing session)
    #[serde(rename = "session.agent.attached")]
    SessionAgentAttached,
    
    /// Agent detached / completed in a session
    #[serde(rename = "session.agent.detached")]
    SessionAgentDetached,
    
    /// Session status changed (Running → Idle → Completed)
    #[serde(rename = "session.status")]
    SessionStatusChanged,
}
```

**Event payload structure:**

```rust
// agent.spawned event body (enhanced)
struct AgentSpawnedBody {
    pid: u32,
    agent_name: String,
    invocation_id: String,
    session_id: String,       // NEW: Avix Session ID
    is_sub_agent: bool,        // NEW: true if spawned into existing session
}

// session.agent.attached event body
struct SessionAgentAttachedBody {
    session_id: String,
    agent_name: String,
    primary_agent: String,    // who is now in focus
    participants: Vec<String>,
}

// session.status event body
struct SessionStatusChangedBody {
    session_id: String,
    old_status: SessionStatus,
    new_status: SessionStatus,
}
```

**Frontend / Sidebar grouping:**

The client can now group events by `session_id` (Avix Session):
1. All events from agents in the same session share the same `session_id`
2. Sidebar shows sessions as expandable groups
3. Each session shows: title, status badge, participant count
4. Events within a session are shown indented under the session

**Implementation:**

Update `event_bus.rs` and the emit points in:
- `ProcHandler::spawn` → emit `AgentSpawned` with session_id
- `RuntimeExecutor::shutdown_with_status` → emit `AgentExit` with session_id
- When sub-agent attaches → emit `SessionAgentAttached`
- When session status changes → emit `SessionStatusChanged`

### Error Handling

- `SessionNotFound` — when `session/get` or `session/resume` finds no matching session
- `SessionNotIdle` — when `session/resume` called on non-Idle session (invalid state)
- `InvocationNotFound` — fallback for missing invocation records
- Use `thiserror` for library errors in `avix-core`

### Alignment with CLAUDE.md Invariants

- **ADR-02**: RuntimeExecutor calls `llm/complete` via IPC — unchanged
- **ADR-04**: Category 2 tools registered at spawn — unchanged
- **ADR-05**: Fresh IPC connection per call — unchanged
- **ADR-06**: Secrets never VFS-readable — unchanged
- **ADR-07**: ApprovalToken is single-use — unchanged

Sessions are a new layer on top, no existing invariants violated.

---

## Testing Requirements

### Unit Tests
- `InvocationStatus::Idle` roundtrip
- `SessionRecord` construction, status transitions, serialization
- `SessionStore` CRUD operations

### Integration Tests
- `kernel/proc/session/create` → returns valid UUID
- `kernel/proc/session/list` → filters by username
- `kernel/proc/session/resume` → spawns new invocation in existing session
- `proc/spawn` without session_id → auto-creates session

### Manual Scenarios
1. Spawn agent → agent completes → becomes Idle
2. Resume session → new invocation attaches to same session
3. Kill invocation while Idle → status becomes Killed
4. Daemon restart → sessions and invocations survive

---

## Usability Considerations

After implementation, the Usability Agent should verify:

1. **TUI**: Can user see session status in agent list? Is Idle distinguishable from Running?
2. **CLI**: `avix session list` shows correct status badges
3. **GUI**: Session page shows all sessions with status, allows resume
4. **Error messages**: Clear feedback when resuming non-Idle session

---

## Estimated Effort & Priority

| Task | Complexity | Effort |
|------|------------|--------|
| Task 1: Idle in InvocationStatus | Low | 1 hour |
| Task 2: SessionRecord type | Medium | 2 hours |
| Task 3: SessionStore persistence | Medium | 3 hours |
| Task 4: Wire into ProcHandler | Medium | 2 hours |
| Task 5: RuntimeExecutor changes | Medium | 2 hours |
| Task 6: ATP event kinds | Low | 1 hour |
| Task 7: ATP proc handlers | Low | 1 hour |
| Task 8: CLI commands | Low | 1 hour |
| Task 9: Testing & backward compat | Medium | 2 hours |

**Total estimate:** ~16 hours

**Priority:** High — unblocks multi-turn agent workflows and history v2 completion.

---

## Completion Checklist

- [ ] `Idle` added to `InvocationStatus`
- [ ] `SessionRecord` and `SessionStatus` defined and tested
- [ ] `SessionStore` implements redb + VFS persistence
- [ ] `ProcHandler` supports session creation and attachment
- [ ] `RuntimeExecutor` handles Idle transition
- [ ] ATP `session-*` operations implemented
- [ ] CLI `session *` commands work
- [ ] Backward compatibility verified
- [ ] All tests pass (`cargo test --workspace`)
- [ ] Zero clippy warnings
- [ ] Zero fmt diffs
- [ ] Usability Agent validates UX

---

## Feedback Integration

This dev plan incorporates:

- **Spec alignment**: All fields from the spec are included in `SessionRecord`
- **Existing patterns**: Uses same redb + LocalProvider pattern as `InvocationStore`
- **Backward compat**: Auto-session creation for empty `session_id` ensures existing code works
- **Naming**: Uses `session_id` (existing field) rather than introducing new terminology

No external feedback received yet.
