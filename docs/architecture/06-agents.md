# 06 — Agents

> RuntimeExecutor, agent spawn, proc file writes, the 7-step turn loop, and signals.

---

## Overview

An agent is an LLM conversation loop managed by a `RuntimeExecutor`. The LLM is stateless;
the `RuntimeExecutor` is the actual OS-level process — it owns the conversation context,
enforces capability policy, manages tool dispatch, and handles all kernel interactions.

The LLM **never sees** raw capability tokens, IPC messages, or signal delivery.
Everything is mediated through the tool interface.

---

## Built-in Agents

| Agent | Description | LLM required | Key capabilities |
|-------|-------------|:---:|---|
| `kernel.agent` | System supervisor. Holds `kernel:root`. | Optional | `kernel:root`, `llm:inference` |
| `planner.agent` | Task decomposition. | Yes | `fs:read`, `llm:inference` |
| `executor.agent` | Tool execution loop. | Yes | `fs:read`, `fs:write`, `exec:*`, `llm:inference` |
| `memory.agent` | File indexing and context retrieval. | Yes | `fs:read`, `llm:inference` |
| `observer.agent` | System health monitoring. | Optional | `fs:read`, `kernel:root` |

All agents live in `/bin/` (system) or `/users/<username>/bin/` (user-installed).

---

## Agent Spawn

An agent is spawned via the `kernel/proc/spawn` syscall. The kernel:

1. Assigns a PID from the process table
2. Issues a `CapabilityToken` (tool grants from crew + user ACL intersection)
3. Creates the `RuntimeExecutor` with the token
4. **Writes `/proc/<pid>/status.yaml` to VFS**
5. **Writes `/proc/<pid>/resolved.yaml` to VFS**
6. Sends `SIGSTART` to the agent

### `/proc/<pid>/status.yaml`

Serialized `AgentStatus`. Written at spawn and updated on every lifecycle event.
All keys are `camelCase` in the YAML output.

```yaml
apiVersion: avix/v1
kind: AgentStatus
metadata:
  name: researcher
  pid: 57
  spawnedAt: "2026-03-22T10:00:00Z"
  spawnedBy: alice
status:
  state: running              # pending | running | paused | waiting | stopped | crashed
  goal: "Research Q3 data"
  contextUsed: 5000
  contextLimit: 200000
  toolCallsThisTurn: 2
  lastActivityAt: "2026-03-22T10:05:30Z"
  waitingOn: null             # null | human-approval | pipe-read | pipe-write | signal
  tools:
    granted: [fs/read, llm/complete]
    denied:  [send_email]
  pipes:
    - id: pipe-001
      targetPid: 58
      direction: out          # in | out | bidirectional
      state: open             # open | closed | draining
  signals:
    lastReceived: SIGSTART
    pendingCount: 0
  metrics:
    tokensConsumed: 14200
    toolCallsTotal: 11
    wallTimeSec: 330
```

**Process states:** `pending` (allocated, not yet started) | `running` | `paused` (SIGPAUSE received)
| `waiting` (blocked on pipe or signal) | `stopped` (SIGKILL received) | `crashed` (runtime error)

### `/proc/<pid>/resolved.yaml`

The merged final configuration this agent runs under. Written at spawn. All keys `camelCase`.

```yaml
apiVersion: avix/v1
kind: Resolved
metadata:
  target: researcher
  resolvedAt: "2026-03-22T10:00:00Z"
  resolvedFor:
    username: alice
    pid: 57
    crews: [researchers]
spec:
  contextWindowTokens: 64000
  maxToolChainLength: 50
  tokenTtlSecs: 3600
  grantedTools:
    - fs/read
    - llm/complete
  annotations: {}
```

**Implementation rule:** These files are written by `RuntimeExecutor::write_proc_files()`
called via `init_proc_files()` after spawn. If no VFS handle is attached, the write is
silently skipped — spawn succeeds regardless.

### `/proc/<pid>/hil-queue/<hil-id>.yaml`

Written by `HilManager::open()` when a HIL event is created. Updated by
`HilManager::resolve()` or timeout. All keys `snake_case` in YAML.

```yaml
api_version: avix/v1
kind: HilRequest
hil_id: hil-abc-001
pid: 57
agent_name: researcher
hil_type: tool_call_approval   # tool_call_approval | capability_upgrade | escalation
tool: send_email
args:
  to: team@org.com
  subject: Summary ready
reason: "Wants to notify user when analysis is complete"
urgency: normal                # low | normal | high
approval_token: "<opaque token — present this in SIGRESUME payload>"
created_at: "2026-03-22T10:05:00Z"
expires_at: "2026-03-22T10:15:00Z"
state: pending                 # pending | approved | denied | timeout
```

Human clients read `approval_token` from this file and include it in the `SIGRESUME`
payload when responding (see `04-atp.md` HIL flow).

---

## Category 2 Tool Registration

At spawn, `RuntimeExecutor` registers Category 2 tools via `ipc.tool-add`:

```
agent/spawn, agent/kill, agent/list
pipe/open, pipe/write, pipe/read, pipe/close
cap/request-tool, cap/escalate, cap/list, cap/list-granted
job/watch
```

These tools are NOT hard-coded in any service's tool list. They are registered by
`RuntimeExecutor` at spawn time and removed via `ipc.tool-remove` at exit. The LLM
always sees an accurate tool list that reflects the agent's actual runtime grants.

**Always-present tools** (regardless of capability grants):
`cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch`

---

## The 7-Step Turn Loop

Each turn of the agent loop:

1. **Receive** — get LLM response (text content blocks + tool call requests)
2. **Validate** — check each requested tool against `CapabilityToken.granted_tools`
3. **HIL check** — if any tool is in `hilRequiredTools` policy list, pause for approval
4. **Dispatch** — send validated tool calls to `router.svc` via IPC
5. **Collect** — gather all tool results
6. **Inject** — add results to conversation context
7. **Continue** — feed updated context back to LLM, or exit if task complete

Category 3 (transparent) behaviours run automatically within the loop:
- Token renewal when expiry is within `renewalWindowSecs`
- HIL pausing on `hilRequiredTools` hits
- Snapshot triggers on `SIGSAVE`
- Tool chain depth enforcement

---

## Signals

Signals are delivered as JSON-RPC notifications on the agent's per-PID socket
(`/run/avix/agents/<pid>.sock`). No response is sent or expected.

| Signal | Direction | Meaning |
|--------|-----------|---------|
| `SIGSTART` | Kernel → Agent | Begin execution |
| `SIGPAUSE` | Kernel → Agent | Suspend at next tool boundary; payload carries `hilId` for HIL pauses |
| `SIGRESUME` | Kernel → Agent | Resume; payload carries HIL decision |
| `SIGKILL` | Kernel → Agent | Terminate immediately |
| `SIGSTOP` | Kernel → Agent | Stop (session closed) |
| `SIGSAVE` | Kernel → Agent | Take a snapshot now |
| `SIGPIPE` | Kernel → Agent | Pipe established/closed |
| `SIGESCALATE` | Agent → Kernel | Agent proactively requests human escalation |

`SIGRESUME` payload (capability upgrade approved):

```json
{
  "hilId": "hil-002",
  "decision": "approved",
  "scope": "session",
  "new_capability_token": "<full new HMAC-signed token>"
}
```

`RuntimeExecutor` replaces its held token when `new_capability_token` is present.

---

## Agent Status Lifecycle

```
kernel.proc/spawn
  → status: running
  → /proc/<pid>/status.yaml written

SIGPAUSE
  → status: paused
  → /proc/<pid>/status.yaml updated

SIGRESUME
  → status: running
  → /proc/<pid>/status.yaml updated

task complete (LLM returns no tool calls)
  → status: completed
  → /proc/<pid>/status.yaml updated

SIGKILL
  → status: stopped
  → token invalidated
  → Category 2 tools deregistered via ipc.tool-remove
  → /proc/<pid>/status.yaml updated
```

---

## Defaults and Limits Resolution Order

For any agent configuration value:

```
/kernel/limits/agent.yaml      (hard ceiling — kernel enforced)
    ↓
/users/<username>/limits.yaml  (per-user ceiling)
    ↓
/users/<username>/defaults.yaml (per-user preference)
    ↓
/kernel/defaults/agent.yaml    (compiled-in system defaults)
```

The merged result is written to `/proc/<pid>/resolved.yaml` at spawn time.

---

## Pipes

Agents communicate via pipes. A pipe is an ordered, backpressure-aware channel between
two agents.

```
pipe/open   → creates /proc/<pid>/pipes/<id>.yaml; SIGPIPE delivered to both ends
pipe/write  → writes content to buffer (blocks if buffer full — backpressure)
pipe/read   → reads from buffer
pipe/close  → closes and cleans up
```

Pipe configuration defaults from `/kernel/defaults/pipe.yaml` (`bufferTokens: 8192`).

---

## Snapshots

`SIGSAVE` triggers a snapshot:
1. RuntimeExecutor serialises full conversation context + tool state
2. Writes to `/users/<username>/snapshots/<agent>-<timestamp>.yaml`
3. Returns `snapshot_id` to kernel

Restore: `kernel/proc/spawn` with `restore_from: <snapshot_id>` reconstructs the context.
