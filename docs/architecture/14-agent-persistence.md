# 14 — Agent Persistence

> Agent discovery (catalog), invocation records, session management, and conversation persistence.

---

## Overview

Avix distinguishes three related but separate concepts:

| Concept | Lifetime | Location |
|---------|----------|----------|
| **Installed agent** | Persistent — survives reboot | `/bin/<name>@<version>/` (system) or `/users/<username>/bin/<name>@<version>/` (user) |
| **Session** | Persistent — survives reboot | `<AVIX_ROOT>/data/users/<username>/sessions/` |
| **Invocation** | Persistent — survives reboot | `<AVIX_ROOT>/data/users/<username>/agents/<agent>/invocations/` |

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
    pub owner_pid: u32,               // PID that created the session — required, always non-zero
    pub pids: Vec<u32>,               // all currently active PIDs in this session
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

Sessions persist via:
- **redb** — fast keyed lookups for `list_sessions` and `get_session`
- **LocalProvider** — YAML manifest at `users/<username>/sessions/<id>/session.yaml`

### Disk layout

```
AVIX_ROOT/users/<username>/sessions/<session_id>/
└── session.yaml              ← SessionRecord summary
```

---

## Invocation Persistence — InvocationStore

`crates/avix-core/src/invocation/`

Every agent spawn creates an `InvocationRecord`. Records persist across reboots via two complementary stores:

- **redb** (primary) — fast queryable key-value store keyed by invocation UUID. Used for `list_invocations` and `get_invocation`.
- **LocalProvider** (secondary) — human-readable YAML summary + JSONL conversation written to `AVIX_ROOT/users/`.

### Disk layout

```
AVIX_ROOT/users/<username>/agents/<agent_name>/invocations/
├── <uuid>.yaml              ← summary (status, tokens, goal, timing)
└── <uuid>/
    └── conversation.jsonl   ← one JSON object per line: {role, content}
```

### InvocationRecord fields

```rust
pub struct InvocationRecord {
    pub id: String,                   // UUID v4
    pub agent_name: String,
    pub username: String,
    pub pid: u32,
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
- The agent signals it's waiting for input (`exit_reason = "waiting_for_input"`)
- The agent is not terminated but paused, awaiting:
  - User input via ATP
  - Another agent's tool call targeting this session
  - External trigger (future)

From `Idle`, a new invocation can be spawned in the same session (continuation) or the session can be explicitly resumed.

### Lifecycle

```
ProcHandler::spawn(name, goal, session_id?, parent_pid?)
  1. Allocate PID (must happen first — used as owner_pid at session creation)
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
  8. Pass invocation_id in SpawnParams → RuntimeExecutor

RuntimeExecutor::shutdown_with_status(status, exit_reason)
  1. Deregister Category 2 tools
  2. If exit_reason == "waiting_for_input":
     - store.update_status(id, Idle)
     - session.mark_idle()
     - return (do NOT finalize)
  3. Otherwise:
     - store.write_conversation(id, username, agent_name, &messages)
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
| `proc/invocation-list` | `kernel/proc/invocation-list` | `{ "username": "alice", "agent_name": "researcher" }` |
| `proc/invocation-get` | `kernel/proc/invocation-get` | `{ "id": "<uuid>" }` |

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

## Client Commands

`crates/avix-client-core/src/commands.rs`:

```rust
pub async fn list_installed(dispatcher, username) -> Result<Vec<Value>>
pub async fn list_invocations(dispatcher, username, agent_name: Option<&str>) -> Result<Vec<Value>>
pub async fn get_invocation(dispatcher, invocation_id) -> Result<Option<Value>>
pub async fn list_sessions(dispatcher, username) -> Result<Vec<SessionRecord>>
pub async fn get_session(dispatcher, session_id) -> Result<Option<SessionRecord>>
pub async fn resume_session(dispatcher, session_id, input) -> Result<InvocationRecord>
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

### Catalog page (existing)
- Lists installed agents

### History page (existing)
- Shows invocation history

### Sessions page (NEW - Phase 2)
- Lists all sessions with status badges
- Expandable to show all invocations in session
- Resume action to continue from Idle state

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
