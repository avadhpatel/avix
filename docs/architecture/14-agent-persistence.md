# 14 — Agent Persistence

> Agent discovery (catalog), invocation records, session management, and conversation persistence.

---

## Overview

Avix distinguishes three related but separate concepts:

| Concept | Lifetime | Location |
|---------|----------|----------|
| **Installed agent** | Persistent — survives reboot | `/bin/<name>@<version>/` (system) or `/users/<username>/bin/<name>@<version>/` (user) |
| **Session** | Persistent — survives reboot | `<AVIX_ROOT>/data/sessions.redb` (index) + per-PID JSONL under `<AVIX_ROOT>/data/users/<username>/.sessions/` |
| **Invocation** | Persistent — survives reboot | `<AVIX_ROOT>/data/invocations.redb` |

An _installed agent_ is a manifest describing an agent that can be spawned. A _session_ is a persistent container for one or more agent invocations working toward a shared goal. An _invocation_ is a single spawn→exit lifecycle — the running record of one execution, including conversation history.

---

## Agent Discovery — ManifestScanner

`crates/avix-core/src/agent_manifest/scanner.rs`

The `ManifestScanner` enumerates all agents available to a given user by scanning two VFS trees:

1. `/bin/` — **System scope** — installed by an operator; available to all users; backed by `AVIX_ROOT/data/bin/`
2. `/users/<username>/bin/` — **User scope** — personal installs; available only that user; backed by `AVIX_ROOT/data/users/<username>/bin/`

**Resolution order / collision rule:** when a user-installed agent has the same `name` as a system agent, the system agent wins and the user entry is silently omitted.

```
ManifestScanner::scan(username)
  └── scan_dir("/bin/", System)           → reads /bin/<name>@<version>/manifest.yaml for each dir
  └── scan_dir("/users/<u>/bin/", User)   → reads /users/<u>/bin/<name>@<version>/manifest.yaml
         (skips names already present in system results)
```
ManifestScanner::scan(username)
  └── scan_dir("/bin/", System)           → reads /bin/<dir>/manifest.yaml for each dir
  └── scan_dir("/users/<u>/bin/", User)   → reads /users/<u>/bin/<dir>/manifest.yaml
         (skips names already present in system results)
```

Each `manifest.yaml` must have `kind: AgentManifest`. Entries that fail to parse are skipped with a `warn!()` log — a malformed manifest never prevents other agents from being discoverable.

### AgentManifestSummary

```rust
pub struct AgentManifestSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub path: String,        // VFS path to the manifest file
    pub scope: AgentScope,   // System | User
}
```

### Admin variant

`scan_all()` scans `/bin/` plus every `/users/*/bin/` directory — used by admin tooling that needs a global view across all users.

---

## Session Management — SessionStore

`crates/avix-core/src/session/`

A **Session** is a persistent, observable container that groups multiple invocations (from one or more agents) working toward a shared goal. Sessions survive daemon restarts and support an **Idle** state for multi-turn agent workflows.

### SessionRecord fields

```rust
pub struct SessionRecord {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,      // for forking/branching sessions
    pub project_id: Option<String>,   // future workspace support
    pub title: String,                // auto or user-provided
    pub goal: String,
    pub username: String,
    pub spawned_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub status: SessionStatus,        // Running | Idle | Paused | Completed | Failed | Archived
    pub summary: Option<String>,      // high-level summary (updated on Idle)
    pub tokens_total: u64,
    pub origin_agent: String,         // agent_name that started the session
    pub primary_agent: String,        // agent_name currently in control
    pub participants: Vec<String>,    // all agent_names involved
    pub owner_pid: u64,               // PID that created the session — required, always non-zero; time-seeded u64 (Pid::generate())
    pub pids: Vec<u64>,               // all currently active PIDs in this session
    pub invocation_pids: Vec<PidInvocationMeta>, // per-PID spawn metadata; populated at spawn time
}

pub struct PidInvocationMeta {
    pub pid: u64,
    pub invocation_id: String,        // UUID of the InvocationRecord for this PID
    pub agent_name: String,
    pub agent_version: String,        // empty string if manifest has no version field
    pub spawned_at: DateTime<Utc>,
}

pub enum SessionStatus {
    Running,
    Idle,        // waiting for input
    Paused,      // all invocations suspended; can be resumed via kernel/proc/session/resume
    Completed,
    Failed,
    Archived,
}
```

### Multi-agent session semantics

- **origin_agent**: The first agent that started the session (set at session creation)
- **primary_agent**: The agent currently in focus / control
- **participants**: All agent names involved in the session (origin + sub-agents)

When an agent spawns a sub-agent into an existing session:
1. The sub-agent is added to `participants`
2. The sub-agent becomes the `primary_agent`
3. When the sub-agent completes, `primary_agent` returns to `origin_agent`

### Session persistence

Sessions persist via a single **redb** store at `AVIX_ROOT/data/sessions.redb`. The `SessionRecord` (including `invocation_pids`) is serialised as JSON and stored keyed by session UUID. No YAML artefacts are written.

### Disk layout

```
AVIX_ROOT/data/sessions.redb            ← SessionRecord JSON, keyed by UUID

AVIX_ROOT/data/users/<username>/.sessions/<session_id>/
└── <pid>.jsonl                          ← conversation JSONL for that PID; reused across turns
```

The `.sessions/` tree is written by `InvocationStore` (not `SessionStore`). Each PID in a session gets its own JSONL file, created on first write and appended on subsequent turns until the process exits.

---

## Invocation Persistence — InvocationStore

`crates/avix-core/src/invocation/`

Every agent spawn creates an `InvocationRecord`. Records persist across reboots via:

- **redb** — fast queryable key-value store at `AVIX_ROOT/data/invocations.redb`, keyed by invocation UUID. Used for `list_invocations` and `get_invocation`.
- **LocalProvider** — JSONL conversation file written under `AVIX_ROOT/data/users/`, at path `<username>/.sessions/<session_id>/<pid>.jsonl`.

No YAML summary files are written. All structured metadata lives in redb.

### Disk layout

```
AVIX_ROOT/data/invocations.redb                          ← InvocationRecord JSON, keyed by UUID

AVIX_ROOT/data/users/<username>/.sessions/<session_id>/
└── <pid>.jsonl                                          ← one ConversationEntry per line
```

The JSONL file is keyed by PID (not invocation UUID) and reused across turns for the lifetime of that process. Multiple PIDs sharing a session each write their own file.

Each line in `conversation.jsonl` is a `ConversationEntry` object. Entries are written
incrementally during `run_with_client` (not only at shutdown):

| When | Entry written |
|------|--------------|
| Turn loop start | `{ role: user, content: goal }` |
| Each tool-dispatch turn | `{ role: assistant, content: text_portion, toolCalls: [{ id, name, args, result }] }` — result is included after the tool returns |
| Final assistant response | `{ role: assistant, content: response_text }` |
| Non-clean exit (Failed/Killed) | `{ role: system, content: "[Agent stopped: <exit_reason>]" }` — appended by `shutdown_with_status` before flushing |

Example JSONL for a two-turn session that hits the tool chain limit:

```json
{"role":"user","content":"Research quantum computing"}
{"role":"assistant","content":"I'll start by searching…","toolCalls":[{"id":"tc-1","name":"fs/read","args":{"path":"/docs"},"result":{"content":"..."}}]}
{"role":"system","content":"[Agent stopped: exceeded max tool chain limit of 8]"}
```

All optional fields (`toolCalls`, `filesChanged`, `thought`) default to empty/null via
`#[serde(default)]` — files written by older versions (flat `{role, content}` pairs)
parse correctly. Lines that fail to deserialize are **skipped with a warning** rather
than aborting the read, so a single corrupt line never makes the whole conversation
unreadable.

### InvocationRecord fields

```rust
pub struct InvocationRecord {
    pub id: String,                   // UUID v4
    pub agent_name: String,
    pub agent_version: String,        // from manifest; empty string if not present
    pub username: String,
    pub pid: u64,                     // time-seeded u64 PID — informational only; not stable across reboots within a session
    pub goal: String,
    pub session_id: String,          // REQUIRED - links to parent session
    pub spawned_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: InvocationStatus,     // Running | Idle | Completed | Failed | Killed
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    pub exit_reason: Option<String>,
}

pub enum InvocationStatus {
    Running,
    Idle,        // waiting for input (agent can be resumed)
    Paused,      // suspended by SIGPAUSE — HIL wait or manual pause; non-terminal
    Completed,
    Failed,
    Killed,
}
```

### Idle state semantics

An invocation transitions to `Idle` when:
- The agent completes a turn and is waiting for the next user message (the executor calls
  `RuntimeExecutor::idle()` before entering `wait_for_next_goal()`)

From `Idle`, a new invocation can be spawned in the same session (continuation) or the
session can be explicitly resumed via `avix session resume <id>`.

### Boot-time crash recovery

On every `avix start`, `phase3_crash_recovery` runs before phase 3 (services) and the
ATP gateway, ensuring no client sees stale records:

| Invocation status | After recovery |
|---|---|
| `Running` | `Killed` (exit_reason: "interrupted_at_shutdown") |
| `Paused` | `Killed` (exit_reason: "interrupted_at_shutdown") |
| `Idle`, `Completed`, `Failed`, `Killed` | unchanged |

| Session status | After recovery |
|---|---|
| `Running` | `Idle` + `pids` cleared |
| `Paused` | `Idle` + `pids` cleared |
| `Idle`, `Completed`, `Failed`, `Archived` | unchanged |

Sessions transitioned to `Idle` remain available for resumption.

### Lifecycle

```
ProcHandler::spawn(name, goal, session_id?, parent_pid?)
  1. Generate PID via `Pid::generate()` — u64 time-seeded (42-bit ms since 2025-01-01 | 22-bit random salt), collision-free across reboots (used as owner_pid at session creation)
  2. Session resolution:
     - If parent_pid provided → inherit parent's session (attach as participant; add_pid(pid))
     - Else if session_id provided → attach to that session (add_pid(pid))
     - Else → create new session via SessionRecord::new(..., owner_pid=pid)
               SessionRecord::new() initialises pids: vec![owner_pid] automatically
  3. active_sessions.insert(pid, session_id)
  4. Generate invocation_id = Uuid::new_v4()
  5. store.create(&InvocationRecord { status: Running, session_id, ... })
  6. active_invocations.insert(pid, invocation_id)
  7. Set ProcessEntry.parent = parent_pid.map(Pid::new)
  8. Add PidInvocationMeta { pid, invocation_id, agent_name, agent_version, spawned_at }
     to session.invocation_pids via session_store.update(session)
  9. Pass invocation_id in SpawnParams → RuntimeExecutor

RuntimeExecutor::shutdown_with_status(status, exit_reason)
  1. Deregister Category 2 tools
  2. If exit_reason == "waiting_for_input":
     - store.update_status(id, Idle)
     - session.mark_idle()
     - return (do NOT finalize)
  3. Otherwise:
     - If exit_reason is set: append System entry "[Agent stopped: <reason>]"
       to MemoryManager.conversation_history
     - store.write_conversation_structured(pid, session_id, username,
         &memory.conversation_history)
       → writes to `<username>/.sessions/<session_id>/<pid>.jsonl`
       (history is built incrementally during run_with_client — see disk layout above)
     - store.finalize(id, status, ended_at, tokens, tool_calls, exit_reason)
     - If sub-agent completed → session.set_primary(origin_agent)

ProcHandler::abort_agent(pid)
  → finalize_invocation(pid, Killed, "killed")
  → finalize_session_for_pid(pid, Killed)

ProcHandler::finalize_session_for_pid(pid, status)
  → session.remove_pid(pid)
  → If pid == session.owner_pid:
      Completed → session.mark_completed()
      Failed | Killed → session.mark_failed()
  → session_store.update(session)

ProcHandler::pause_agent(pid)
  → process_table.set_status(pid, Paused)
  → invocation_store.update_status(inv_id, Paused)
  → SignalDelivery.deliver(SIGPAUSE, pid)
  → If pid == session.owner_pid:
      broadcast SIGPAUSE to all other session PIDs
      update each sibling invocation to Paused
      session.mark_paused()
      session_store.update(session)

ProcHandler::resume_agent(pid)
  → process_table.set_status(pid, Running)
  → invocation_store.update_status(inv_id, Running)
  → SignalDelivery.deliver(SIGRESUME, pid)
  → If session.status == Paused:
      broadcast SIGRESUME to all other session PIDs
      update each sibling invocation to Running
      session.mark_running()
      session_store.update(session)
```

### Session pause / resume via IPC

| IPC method | Behavior |
|---|---|
| `kernel/proc/pause` (pid) | `pause_agent(pid)` — cascades to whole session if pid is owner |
| `kernel/proc/resume` (pid) | `resume_agent(pid)` — cascades to whole session if session is Paused |
| `kernel/proc/session/pause` | Calls `pause_agent(session.owner_pid)` — full cascade guaranteed |
| `kernel/proc/session/resume` | If session is Paused + has active PIDs → SIGRESUME all PIDs; else spawn new invocation |

---

## ATP Interface

### Agent ops

| ATP op | IPC method | Body |
|--------|------------|------|
| `proc/list-installed` | `kernel/proc/list-installed` | `{ "username": "alice" }` |
| `proc/invocation-list` | `kernel/proc/invocation-list` | `{ "username": "alice", "agent_name": "researcher" }` or `{ "session_id": "<uuid>" }` |
| `proc/invocation-get` | `kernel/proc/invocation-get` | `{ "id": "<uuid>" }` |
| `proc/invocation-conversation` | `kernel/proc/invocation-conversation` | `{ "id": "<uuid>" }` |

`invocation-list` selects by session when `session_id` is present; otherwise falls back to the `username`/`agent_name` filter. The gateway handler in `gateway/handlers/proc.rs` passes `session_id` through transparently.

`invocation-conversation` returns the parsed `conversation.jsonl` for the given invocation as a JSON array of `ConversationEntry` objects. Returns an empty array if no conversation file exists yet (pre-first-turn invocation).

### Session ops

Sessions are **created exclusively by the kernel** during `kernel/proc/spawn` — there is no
create endpoint. External callers can only observe and resume sessions.

| ATP op | IPC method | Body |
|--------|------------|------|
| `proc/session-list` | `kernel/proc/session/list` | `{ "username": "alice" }` |
| `proc/session-get` | `kernel/proc/session/get` | `{ "id": "<uuid>" }` |
| `proc/session-resume` | `kernel/proc/session/resume` | `{ "session_id": "<uuid>", "input": "..." }` |

All ops forward via `ipc_forward()` in the gateway proc handler.

---

## InvocationStore — Read Methods

In addition to the write methods, `InvocationStore` exposes the following read methods:

```rust
pub async fn list_for_user(&self, username: &str) -> Result<Vec<InvocationRecord>>
pub async fn list_for_agent(&self, username: &str, agent_name: &str) -> Result<Vec<InvocationRecord>>
pub async fn list_for_session(&self, session_id: &str) -> Result<Vec<InvocationRecord>>
pub async fn get(&self, id: &str) -> Result<Option<InvocationRecord>>
pub async fn list_all(&self) -> Result<Vec<InvocationRecord>>

/// Read the persisted conversation.jsonl for an invocation.
/// Returns an empty vec if the file does not exist (pre-first-turn invocations).
pub async fn read_conversation(
    &self,
    id: &str,
    username: &str,
    agent_name: &str,
) -> Result<Vec<ConversationEntry>>
```

`list_for_session` does a full-scan of the redb table and filters by `record.session_id`.
`read_conversation` builds the path `users/<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl`, reads via `LocalProvider`, and deserializes each line as a `ConversationEntry`. Lines that fail to deserialize are **skipped with a `warn!` log** — a single corrupt line never prevents the rest of the conversation from being returned.

## ProcHandler — Conversation Access

`ProcHandler` exposes two delegation methods for the IPC server:

```rust
pub async fn list_invocations_for_session(&self, session_id: &str) -> Result<Vec<InvocationRecord>>
pub async fn read_invocation_conversation(&self, invocation_id: &str) -> Result<Vec<ConversationEntry>>
```

`read_invocation_conversation` resolves `username` and `agent_name` from the stored record before calling `InvocationStore::read_conversation`.

---

## Client Commands

`crates/avix-client-core/src/commands.rs`:

```rust
pub async fn list_installed(dispatcher, username) -> Result<Vec<Value>>
pub async fn list_invocations(dispatcher, username, agent_name: Option<&str>) -> Result<Vec<Value>>
pub async fn list_invocations_live(dispatcher, username, agent_name: Option<&str>) -> Result<Vec<Value>>
pub async fn list_invocations_for_session(dispatcher, session_id: &str) -> Result<Vec<Value>>
pub async fn get_invocation(dispatcher, invocation_id) -> Result<Option<Value>>
pub async fn get_invocation_conversation(dispatcher, invocation_id: &str) -> Result<Vec<Value>>
pub async fn list_sessions(dispatcher, username) -> Result<Vec<Value>>
pub async fn get_session(dispatcher, session_id) -> Result<Option<Value>>
pub async fn resume_session(dispatcher, session_id, input) -> Result<Value>
```

---

## CLI Subcommands

```bash
# Agent commands
avix agent catalog [--username alice]
avix agent history [--agent researcher] [--username alice]
avix agent show <invocation-id>

# Session commands
avix session list [--username alice] [--status idle|running|completed]
avix session show <session-id>
avix session resume <session-id> --input "Continue from where we left off"
```

Sessions cannot be created directly from the CLI — they are created automatically by the kernel
when an agent is spawned without a `parent_pid`.

Output formats:
- Default: human-readable table / YAML
- `--json`: raw JSON array / object

---

## TUI

### Running tab (existing)
- Shows active agents and their current status
- `Idle` agents displayed with distinct indicator

### Catalog tab (existing)
- Lists installed agents

### Sessions tab (NEW - Phase 2)
- Lists all sessions for the user
- Shows session title, status badge, participant count
- Click to expand and see invocations within the session
- Resume button to spawn new invocation in session

---

## GUI (avix-app)

### Session-centric layout

The avix-app web UI is organised around sessions rather than raw agent PIDs.

**Sidebar** — Sessions section replaces the old Agents section:
- Lists active sessions (`Running`, `Idle`, `Paused`) for the authenticated user
- Each item: status dot (green=running, amber=idle/paused), title/goal preview, HIL count badge
- Empty state: "No active sessions — click + to start one"
- **"New Session" (+) button** at the top opens `NewSessionModal`
- Bottom nav: Catalog, History, Services, Tools (unchanged)

**NewSessionModal** — two-step wizard:
1. **Agent picker** — shows all installed agents (calls `list_installed`); search by name/description; scope badge (`SYS`/`USR`)
2. **Goal input** — pre-filled from agent description; textarea; "Start Session" calls `spawn_agent`; auto-navigates to the new session by matching `session.ownerPid === pid`

**SessionPage** — per-session conversation view:
- Header: title/goal, status badge, token count, multi-agent rail toggle
- Message thread: per-invocation blocks, each labelled with agent name; historical entries from `get_session_messages`; live streaming block from `agent.output.chunk`
- **Live tool activity feed**: rendered above the streaming block; shows the last 10 `agent.tool_call` / `agent.tool_result` events for the active pid as a compact monospace ticker (call → yellow, result → green)
- **Auto-reload on exit**: when `agent.exit` fires, `AppContext` increments `conversationVersion`; `SessionPage` depends on this value and re-fetches `get_session_messages` automatically — so the full structured conversation (written by `shutdown_with_status` before the exit event) appears without a manual refresh
- Context-aware input bar:
  - `idle` → textarea → `resume_session(session_id, input)`
  - `running` → textarea → `pipe_text(pid, text)`
  - pending HIL → `HilInlineCard`
  - `completed`/`failed` → read-only + "Spawn new session" button
- Optional collapsible multi-agent rail (visible when `participants.length > 1`)

### Catalog page
- Lists installed agents
- **Spawn** button opens `NewSessionModal` with the agent pre-selected (step 2)
- `AddAgentModal` has been removed — `NewSessionModal` is the single entry point for session creation

### History page (unchanged)
- Shows invocation history table with detail drawer

---

## Invariants

- **Session ↔ Invocation**: Every invocation belongs to exactly one session (`session_id` is required).
- **Session creation is kernel-only**: Sessions are created during `ProcHandler::spawn()`. There is no IPC or ATP endpoint to create a session. `owner_pid` is always a valid non-zero PID — it is set from the newly-allocated PID at session construction and is immutable thereafter.
- **Idle state**: An `Idle` invocation can be resumed; a `Completed`/`Failed`/`Killed` invocation cannot.
- **Multi-agent tracking**: `origin_agent` never changes; `primary_agent` tracks current focus.
- Invocation records survive daemon restart (redb is disk-backed).
- `/users/<username>/agents/` and `/users/<username>/sessions/` are written by the kernel via `LocalProvider` directly — they do not go through the VFS ACL layer (kernel is trusted).
- A `Killed` status is always written when `abort_agent()` is called, even if the executor already exited.
- Spawning without `session_id` or `parent_pid` auto-creates a new session (origin = agent name).
