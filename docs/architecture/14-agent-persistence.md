# 14 — Agent Persistence

> Agent discovery (catalog), invocation records, and conversation persistence.

---

## Overview

Avix distinguishes two related but separate concepts:

| Concept | Lifetime | Location |
|---------|----------|----------|
| **Installed agent** | Persistent — survives reboot | `/bin/<name>/` (system) or `/users/<username>/bin/<name>/` (user) |
| **Invocation** | Persistent — survives reboot | `<AVIX_ROOT>/users/<username>/agents/<agent>/invocations/` |

An _installed agent_ is a manifest describing an agent that can be spawned. An _invocation_ is a single spawn→exit lifecycle — the running record of one execution, including conversation history.

---

## Agent Discovery — ManifestScanner

`crates/avix-core/src/agent_manifest/scanner.rs`

The `ManifestScanner` enumerates all agents available to a given user by scanning two VFS trees:

1. `/bin/` — **System scope** — installed by an operator; available to all users.
2. `/users/<username>/bin/` — **User scope** — personal installs; available only to that user.

**Resolution order / collision rule:** when a user-installed agent has the same `name` as a system agent, the system agent wins and the user entry is silently omitted.

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
    pub session_id: String,
    pub spawned_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: InvocationStatus,     // Running | Completed | Failed | Killed
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    pub exit_reason: Option<String>,
}
```

### Lifecycle

```
ProcHandler::spawn()
  1. Generate invocation_id = Uuid::new_v4()
  2. store.create(&InvocationRecord { status: Running, ... })
  3. active_invocations.insert(pid, invocation_id)
  4. Pass invocation_id in SpawnParams → RuntimeExecutor

RuntimeExecutor::shutdown_with_status(status, exit_reason)
  1. Deregister Category 2 tools
  2. store.write_conversation(id, username, agent_name, &messages)
  3. store.finalize(id, status, ended_at, tokens, tool_calls, exit_reason)

ProcHandler::abort_agent(pid)
  → finalize_invocation(pid, Killed, "killed")
```

---

## ATP Interface

Three new ops on the `proc` domain, forwarded by the gateway → kernel IPC:

| ATP op | IPC method | Body |
|--------|------------|------|
| `proc/list-installed` | `kernel/proc/list-installed` | `{ "username": "alice" }` |
| `proc/invocation-list` | `kernel/proc/invocation-list` | `{ "username": "alice", "agent_name": "researcher" }` (agent_name optional) |
| `proc/invocation-get` | `kernel/proc/invocation-get` | `{ "id": "<uuid>" }` |

All three forward via `ipc_forward()` in the gateway proc handler — no special gateway logic.

---

## Client Commands

`crates/avix-client-core/src/commands.rs`:

```rust
pub async fn list_installed(dispatcher, username) -> Result<Vec<Value>>
pub async fn list_invocations(dispatcher, username, agent_name: Option<&str>) -> Result<Vec<Value>>
pub async fn get_invocation(dispatcher, invocation_id) -> Result<Option<Value>>
```

---

## CLI Subcommands

```bash
# List installed agents available to the current user
avix agent catalog [--username alice]

# List invocation history (optionally filtered by agent)
avix agent history [--agent researcher] [--username alice]

# Show a specific invocation: summary + conversation
avix agent show <invocation-id>
```

Output formats:
- Default: human-readable table / YAML
- `--json`: raw JSON array / object

---

## TUI

The TUI adds a **Catalog tab** alongside the existing Running tab:

- `Tab` key cycles between `Running` and `Catalog` tabs.
- `:catalog` command switches to the Catalog tab and re-fetches from the server.
- The catalog is fetched automatically on connect.
- `↑↓` navigates the catalog list when the Catalog tab is active.

---

## GUI (avix-app)

Two new pages accessible from the sidebar:

### Catalog page
- Lists all installed agents with `[SYS]`/`[USR]` scope badge, version, description.
- Search filter by name or description.
- **Spawn** button on each card opens the `AddAgentModal` pre-filled with the agent name.

### History page
- Table of all invocations: ID (truncated), Agent, Status (colored badge), Spawned, Tokens, Goal.
- Agent name filter re-fetches from server.
- Click any row → slide-in **detail drawer** showing full meta (tokens, tool calls, exit reason) and the conversation messages rendered by role.

---

## Invariants

- Invocation records survive daemon restart (redb is disk-backed).
- `/users/<username>/agents/` is written by the kernel via `LocalProvider` directly — it does not go through the VFS ACL layer (kernel is trusted).
- The session → agent connection is advisory only: `session_id` is stored but sessions themselves remain ephemeral in `/proc/`.
- A `Killed` status is always written when `abort_agent()` is called, even if the executor already exited.
