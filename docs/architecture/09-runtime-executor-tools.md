# 09 — RuntimeExecutor Tool Exposure Model

> **Source of truth:** `docs/spec/runtime-exec-tool-exposure.md`
> This document is the merged/annotated architecture view. Keep both in sync.

---

## Overview

`RuntimeExecutor` mediates between the LLM (stateless — it knows only tool names and
schemas) and the Avix system. It is the stateful component that:

- Owns the agent's message history and turn budget
- Controls which tools the LLM can see and call
- Validates every tool call against the agent's `CapabilityToken`
- Enforces per-tool budgets and the maximum tool-chain length
- Handles Human-in-the-Loop (HIL) escalation (3 scenarios)
- Registers and deregisters Category 2 tools at spawn/exit
- Receives and reacts to kernel signals via an in-process `mpsc` channel

`RuntimeExecutor` never calls provider APIs directly — all AI calls go through
`llm.svc` via IPC (ADR-02).

### In-process signal channel

Each `RuntimeExecutor` owns a `tokio::sync::mpsc` channel pair:

| Field | Type | Purpose |
|---|---|---|
| `signal_tx` | `mpsc::Sender<Signal>` | Exposed via `signal_sender()` — given to `IpcExecutorFactory` at spawn so `SignalHandler` can reach this executor |
| `signal_rx` | `Option<mpsc::Receiver<Signal>>` | Taken by `run_with_client` via `Option::take`; **restored before returning** so `wait_for_next_goal` can use it |

`deliver_signal(&str)` — a convenience method that both updates atomics immediately
(for between-turn polling) AND sends on `signal_tx` (for mid-LLM interruption). Only
`signal_rx` is consumed inside `run_with_client`; the atomics are the source of truth
between turns.

### Multi-turn executor loop

`IpcExecutorFactory::launch()` runs the executor in a persistent `loop {}` rather than
a single shot. After each successful `run_with_client` call:

```
loop {
    run_with_client(goal) → Ok(result)
        │  ← run_with_client restores signal_rx before returning
        │  ← interim invocation state already persisted inside run_with_client
        ↓
    executor.idle()          ← invocation status → Idle, session status → Idle
    event_bus.agent_output   ← emit result to ATP clients
    process_table → Waiting

    wait_for_next_goal()     ← blocks on signal_rx waiting for SIGSTART
        │  SIGSTART{payload.goal} → Some(new_goal)
        │  SIGKILL / SIGSTOP     → None  (break)
        ↓
    (loop back with new_goal)
}
signal_channels.unregister(pid)   ← on exit
```

**SIGSTART payload:** `{ "goal": "<next user message>" }` — delivered by the kernel when
a user sends a follow-up message to a waiting agent (via `proc/session-resume` or
`agent/send-message`).

### Idle vs shutdown

| Method | Deregisters Cat2 tools | Flushes conversation | Updates status |
|---|---|---|---|
| `idle()` | No | No (already persisted) | Invocation → Idle, Session → Idle |
| `shutdown_with_status(status, reason)` | Yes | Yes (full finalize) | Invocation → status |

`idle()` is called after every successful turn — the executor stays alive. `shutdown_with_status`
is called only on error/kill — the executor is about to exit.

---

## Four Tool Categories

### Category 1: Direct Service Tools

Registered by built-in services and the kernel at boot, or by third-party services via
`ipc.tool-add`. The LLM can call them if the agent's `CapabilityToken` grants access.

`RuntimeExecutor` dispatches the call directly over a fresh Unix socket connection
(ADR-05) to the service identified by the tool's `IpcBinding.endpoint`. The JSON-RPC
method called is `IpcBinding.method`. If the service is caller-scoped, `_caller` (PID +
session ID) is injected into the request params before sending.

**`fs/*` tools** route to `KernelIpcServer` via `endpoint: "kernel"` (resolves to
`AVIX_KERNEL_SOCK` / `runtime_dir/kernel.sock`). `KernelIpcServer` delegates to its
`Arc<VfsRouter>` instance, which is passed in at bootstrap Phase 2.

**`kernel/*` syscall tools** have no `IpcBinding` and are forwarded directly to the
kernel IPC server socket with `_caller` always injected.

**Execute permission check:** before dispatching, `RuntimeExecutor` verifies the agent's
`spawned_by` user has execute permission on the tool. Tool owner has implicit execute;
other users need `permissions.all` to contain `x`; `root` is always allowed.
`ToolPermissions` is loaded from the tool's `*.tool.yaml` descriptor at scan time
(see `docs/architecture/07-services.md` § Tool Descriptor Files for derivation rules).

**Built-in Cat1 registrations** (performed by `bootstrap::phase3_services`):

| Namespace       | Tools registered at boot                                     | IPC endpoint | Registered by      |
|-----------------|--------------------------------------------------------------|--------------|--------------------|
| `fs/`           | `read`, `write`, `list`, `exists`, `delete`                  | `kernel`     | `bootstrap` (fixed)|
| `llm/`          | `complete`, `embed`, `generate-image`, `generate-speech`, `transcribe` | `llm` | `bootstrap` (if `etc/llm.yaml` present) |
| `exec/`         | `run` (params: `runtime`, `code`)                            | `exec`       | `bootstrap` (fixed)|
| `mcp/<server>/` | any tool from a connected MCP server                         | `mcp-bridge` | `mcp-bridge.svc`   |

Category 1 tools use `ToolVisibility::All` unless the owning service declares otherwise.

#### Cat1 Descriptor Discovery

At the start of each turn, `RuntimeExecutor::refresh_tool_list()` fetches descriptors
for all Cat1 tools granted in the token (those not in the Cat2 set) by calling
`ToolRegistry::lookup(name)` on the real kernel `ToolRegistry`. Found descriptors are
merged into the LLM's `tools[]` array alongside Cat2 descriptors.

This makes granted Cat1 tools (e.g. `fs/read`, `llm/complete`, `exec/run`) automatically
visible in the LLM context — only tools explicitly granted in the token are included.

The registry is populated at Phase 3 boot:
- `fs/*` (5 tools) — always registered; `endpoint: "kernel"`
- `exec/run` — always registered; `endpoint: "exec"`
- `llm/*` (5 tools) — registered only if `etc/llm.yaml` is present; `endpoint: "llm"`
- `kernel/*` syscalls — registered from `SyscallRegistry` at Phase 3 start

If the registry does not hold a descriptor for a granted Cat1 tool (e.g. `llm.yaml` is
absent), the tool is omitted from `tools[]` that turn. The agent can still call it by
name, but the LLM will not auto-suggest it.

**Wiring:** `IpcExecutorFactory` holds `Arc<Mutex<Option<Arc<ToolRegistry>>>>`. The
registry is `None` at factory construction (phase 2). Phase 3 calls
`factory.set_tool_registry(registry)` after the real registry is built. Spawned
executors receive `RegistryRef::Real(Arc<ToolRegistry>)` when the registry is
available, or fall back to `RegistryRef::Mock` (no Cat1 lookups) otherwise.

#### Per-Agent Tool State (available vs unavailable)

When an agent reads `/tools/<namespace>/<tool>.yaml` from the VFS, the `state:` field
reflects whether the agent holds the required capability grant:

```yaml
state: available      # token.has_tool(cap) == true for all capabilities_required
state: unavailable    # token is missing one or more capabilities_required
```

Unavailable tools also include `request_access: cap/request-tool` so the LLM knows how
to ask for access.

This filtering is implemented in `VfsRouter::generate_tool_yaml()` which reads a
`VfsCallerContext` from `VfsRouter.caller` (a `RwLock`). The context is set per-agent by
`RuntimeExecutor::init_vfs_caller()`:

```rust
pub async fn init_vfs_caller(&self) {
    let Some(vfs) = &self.vfs else { return };
    let ctx = VfsCallerContext {
        username: self.spawned_by.clone(),
        crews: vec![],       // crew info not needed for tool state
        is_admin: false,     // capability grants do the real check
        token: Some(self.token.clone()),
    };
    vfs.set_caller(Some(ctx)).await;
}
```

`init_vfs_caller()` is called from `IpcExecutorFactory::launch()` immediately after
`executor.with_vfs(Arc::clone(&vfs))`. The shared `VfsRouter` is passed into
`IpcExecutorFactory` at construction via `.with_vfs(Arc::clone(&self.vfs))`.

### Category 2: Avix Behaviour Tools

Control the agent's own runtime state. These tools are **not** hard-coded in any
service's tool list. Instead, `RuntimeExecutor` registers them via `ipc.tool-add` at
agent spawn time and removes them via `ipc.tool-remove` at exit (ADR-04).

The set of Category 2 tools granted to an agent depends on its `CapabilityToken`
(see Capability-to-Tool Mapping below).

#### Full Category 2 tool list

| Tool                 | Capability key  | Description                                         |
|----------------------|-----------------|-----------------------------------------------------|
| `agent/spawn`        | `agent:spawn`   | Spawn a child agent with a given goal               |
| `agent/kill`         | `agent:spawn`   | Terminate a child agent by PID                      |
| `agent/list`         | `agent:spawn`   | List agents running in this session (by status)     |
| `agent/wait`         | `agent:spawn`   | Block until a specific child agent completes        |
| `agent/send-message` | `agent:spawn`   | Send a message to another agent via its input pipe  |
| `pipe/open`          | `pipe:use`      | Open a bidirectional IPC pipe to another agent      |
| `pipe/write`         | `pipe:use`      | Write a message to an open pipe                     |
| `pipe/read`          | `pipe:use`      | Read the next message from an open pipe             |
| `pipe/close`         | `pipe:use`      | Close an open pipe                                  |
| `cap/request-tool`   | *(always)*      | Request a capability expansion (triggers HIL)       |
| `cap/escalate`       | *(always)*      | Escalate a decision to a human approver             |
| `cap/list`           | *(always)*      | List all currently granted capabilities             |
| `job/watch`          | *(always)*      | Subscribe to progress events for a job              |
| `sys/tools`          | *(always)*      | Discover available tools by namespace or keyword    |

The five always-present tools (`cap/request-tool`, `cap/escalate`, `cap/list`,
`job/watch`, `sys/tools`) are registered regardless of the agent's capability grants
(Architecture Invariant 13). They also **bypass the capability grant check** in
`validate_tool_call` via the `ALWAYS_PRESENT` constant in
`executor/validation.rs` — an agent can always call them even if its
`CapabilityToken.granted_tools` does not explicitly list them.

`sys/tools` is a discovery tool: rather than flooding the LLM context with every
registered tool on every turn, agents call `sys/tools` to list what is available
(optionally filtered by namespace, keyword, or `granted_only: true`) before requesting
access via `cap/request-tool`.

### Category 3: Transparent RuntimeExecutor Behaviours

These are things `RuntimeExecutor` handles automatically. The LLM never sees them,
never calls them, and is not aware they are happening.

The transparent behaviours `RuntimeExecutor` handles automatically include:

- Tool list refresh on `tool.changed` events
- HIL tool call approval gating (`hilRequiredTools`)
- CapabilityToken renewal before expiry
- Stop-reason detection and context summarisation on `max_tokens`
- Snapshot triggers

### Category 4: MCP-Bridge Tools

Tools from connected MCP servers, proxied by `mcp-bridge.svc`. Registered dynamically
as MCP servers connect/disconnect via `ipc.tool-add`/`ipc.tool-remove`. The LLM calls
them like any Cat1 tool; `RuntimeExecutor` routes them to `router.svc` → `mcp-bridge.svc`.

- Namespace: `mcp/<server>/` (e.g. `mcp/github/list-prs`)
- Wire form: `mcp__github__list-prs` (standard `__` mangling)
- Capability: per-tool grant in `CapabilityToken.granted_tools` (e.g. `"mcp/github/list-prs"`)
- Registered/removed by `mcp-bridge.svc` — never by `RuntimeExecutor` directly

---

## Capability-to-Tool Mapping

`CapabilityToken.granted_tools` stores **individual tool names** (e.g. `"agent/spawn"`,
`"fs/read"`). Capability group names like `agent:spawn` are used only by token issuers
to expand into the individual tools to grant — they never appear in the token itself.

`CapabilityToolMap` maps capability group names to Cat2 tool names for issuers.
`compute_cat2_tools` uses `all_gated_cat2_tools()` to check which Cat2 tools are in a
token, matching each tool name individually.

**`CapabilityToolMap` is Cat2-only.** `llm/complete` and other `llm/*` tools are Cat1
service tools dispatched via `router.svc → llm.svc`. They are granted individually in
`CapabilityToken.granted_tools` but are NOT listed in `CapabilityToolMap` — listing
them there would incorrectly treat them as Cat2 and register them via `ipc.tool-add`.

```
Capability key       → Cat2 tool names stored in granted_tools
─────────────────────────────────────────────────────────────────────
agent:spawn          → agent/spawn, agent/kill, agent/list, agent/wait, agent/send-message
pipe:use             → pipe/open, pipe/write, pipe/read, pipe/close
memory:read          → memory/retrieve, memory/get-fact, memory/get-preferences
memory:write         → memory/retrieve, memory/get-fact, memory/get-preferences,
                       memory/log-event, memory/store-fact, memory/update-preference,
                       memory/forget
memory:share         → memory/share-request
(always, no check)   → cap/request-tool, cap/escalate, cap/list, job/watch, sys/tools
```

At spawn, `RuntimeExecutor` iterates `all_gated_cat2_tools()` and checks
`token.has_tool(name)` for each. Only matching names are registered as Cat2.

Cat1 tools (e.g. `fs/read`, `llm/complete`) in `granted_tools` that are NOT in the Cat2
set are resolved differently — see "Cat1 Descriptor Discovery" below.

---

## Category 2 Registration Lifecycle

```
1. RuntimeExecutor::spawn_with_registry_ref(params, RegistryRef::Real(registry))
   │                                               (or RegistryRef::Mock for tests)
   ├─ compute Category 2 set from token + CapabilityToolMap
   │
   ├─ for each tool in set:
   │    registry.add(tool, schema, Category2, ToolVisibility::User(username))
   │    ipc.tool-add → router.svc
   │
   └─ LLM system prompt built; first turn begins

2. (agent running — tool calls dispatched normally; invocation persisted after each tool call)

2a. turn completes → executor.idle()
    │
    ├─ invocation_store.update_status(id, Idle)
    ├─ session_store: session.mark_idle() + update
    │
    └─ executor loops back; wait_for_next_goal() blocks on signal_rx

2b. SIGSTART received → new goal; loop continues at step 2

3. RuntimeExecutor::shutdown_with_status(status, exit_reason)   ← on error/kill only
   │
   ├─ for each registered Category 2 tool:
   │    registry.remove(tool, drain: true)   ← waits for in-flight calls
   │    ipc.tool-remove → router.svc
   │
   ├─ invocation_store.write_conversation_structured + finalize(status)
   │
   └─ process table entry cleared
```

**`drain: true`** ensures that any in-flight call to a Category 2 tool completes before
the tool is removed from the registry.

---

## Tool Visibility

`ToolVisibility::User(username)` scopes a Category 2 tool to the agent's owning user.
Other users' agents cannot discover or call it via `router.svc`.

Category 1 tools use `ToolVisibility::All` unless the registering service explicitly
restricts them.

---

## System Prompt Block Construction

Before the first turn, `RuntimeExecutor` builds the system prompt from four blocks.
Tool schemas are passed separately in the `tools[]` field of the `llm/complete` call —
the system prompt provides behavioral guidance, not a tool list.

### Block 1 — Identity (static, set at spawn)

```
You are <agent_name>, an AI agent running inside Avix.
Your goal: <goal>
Session: <session_id> | PID: <pid> | User: <spawned_by>
```

### Block 2 — Available Tools (dynamic, rebuilt on `tool.changed` events)

Lists every tool currently available to the agent with name and description.
Rebuilt whenever a `tool.changed` event fires so the LLM never attempts a
currently unavailable tool. Tool schemas are still passed separately in `tools[]`
of the `llm/complete` call — Block 2 provides the human-readable summary.

```
# Available Tools
- **fs/read**: Read the contents of a file
- **agent/spawn**: Spawn a child agent to work on a sub-task
- ...
When you need a tool not listed here, call cap/request-tool.
When you encounter a situation requiring human judgment, call cap/escalate.
When your task is complete, respond with your final answer.
```

### Block 3 — Constraints (static, set at spawn)

```
Max tool calls per turn: <N>
Tool call budgets:
  <tool>: <N> use(s) remaining   (only if non-empty)
```

### Block 4 — Pending Instructions (dynamic, injected by RuntimeExecutor)

Populated at runtime when events occur mid-session:

- HIL escalation guidance: `[Human guidance]: Exclude salary data entirely.`
- HIL denial feedback: `[Human]: Don't send to that address.`
- Tool availability change: `[System]: mcp/github is currently unavailable`
- Memory summary: `[Context summary]: Earlier you found...`

---

## The 7-Step Turn Loop

```
┌─────────────────────────────────────────────────┐
│  1. Refresh tool list                           │
│     Category 1: descriptors fetched from        │
│       ToolRegistry for each granted_tools entry │
│       not in the Cat2 set                       │
│   + Category 2 (registered at spawn)            │
│   + MCP tools (registered by mcp-bridge.svc)    │
│     → exclude tools flagged unhealthy           │
│                                                  │
│  2. Call llm/complete via IPC                   │
│     messages = full history                      │
│     system   = assembled blocks 1–4             │
│     tools    = translated descriptors            │
│                                                  │
│  3. Interpret stop_reason                       │
│     "end_turn"   → return result to caller       │
│     "tool_use"   → proceed to step 4            │
│     "max_tokens" → summarise history, loop back  │
│     "stop_seq"   → treat as end_turn            │
│                                                  │
│  4. Validate each tool call                     │
│     token.has_tool(name)?  → else EPERM         │
│     (always-present tools bypass this check)    │
│     budget[name] > 0?      → else EBUDGET       │
│     → decrement budget atomically               │
│                                                  │
│  5. HIL approval check                          │
│     tool in hilRequiredTools?                   │
│     YES → ResourceRequest tool_call_approval    │
│           → suspend (SIGPAUSE)                  │
│           → await SIGRESUME                     │
│           → denied: inject error, continue      │
│           → approved: proceed to (6)            │
│                                                  │
│  6. Dispatch tool calls                         │
│     is_cat2_tool(name)?                         │
│       YES → RuntimeExecutor handles locally     │
│       NO  → forward to router.svc via IPC       │
│                                                  │
│  7. Append tool results to message history      │
│     Loop back to step 1                         │
│     Until stop_reason = "end_turn"              │
│     or tool_chain_length >= max_tool_chain_length│
└─────────────────────────────────────────────────┘
```

---

## Human-in-the-Loop (HIL) — Three Scenarios

### Scenario 1: Tool Call Approval (automatic intercept)

When the LLM calls a tool that is in its CapabilityToken but flagged in `hilRequiredTools`
(configured in `kernel.yaml`), `RuntimeExecutor` intercepts before dispatching:

```
LLM calls send_email { to: "team@org.com", ... }
  → token.has_tool("send_email") == true ✓
  → "send_email" in hilRequiredTools
  → ResourceRequest { resource: tool_call_approval, tool, args } to kernel
  → kernel mints ApprovalToken, sends SIGPAUSE
  → RuntimeExecutor suspends (does NOT call router.svc yet)
  → human approves / denies via HIL
  → SIGRESUME:
      approved → dispatch via router.svc, inject result, resume
      denied   → inject error result, resume
```

The LLM never knows the HIL gate existed.

### Scenario 2: Capability Expansion Request

The LLM calls `cap/request-tool` to request access to a tool it does not currently hold.
`RuntimeExecutor` suspends the turn (emits `SIGPAUSE`), routes the request to the
configured `human_channel`, and waits for an `ApprovalToken`.

- **Approved**: capability added to token; tool list refreshed; turn resumes from step 1.
- **Rejected**: rejection reason injected via Block 4; turn resumes without the tool.

### Scenario 3: Explicit Escalation

The LLM calls `cap/escalate` to ask a human to make a decision. `RuntimeExecutor`
suspends, delivers the escalation to all `human_channel` tools simultaneously. The
first valid human response atomically invalidates all others (ADR-07).

The human's response is injected into Block 4. The turn resumes.

---

## Category 2 Tool Schemas

### `agent/spawn`

```json
Input:
{
  "agent": "researcher",
  "goal": "Find revenue figures for Q3 2025 from SEC filings",
  "capabilities": ["web", "read"],
  "waitForResult": false
}
Output:
{
  "pid": 58,
  "status": "running",
  "result": null
}
```

### `agent/list`

```json
Input:  { "status": "running" }
Output: { "agents": [{ "pid": 58, "name": "researcher", "status": "running", "goal": "...", "spawnedBy": 57 }] }
```

### `agent/wait`

```json
Input:  { "pid": 58, "timeoutSec": 300 }
Output: { "pid": 58, "finalStatus": "completed", "result": "...", "durationSec": 42 }
```

### `agent/send-message`

```json
Input:  { "pid": 59, "message": "Research done. Revenue: $4.2B." }
Output: { "delivered": true }
```

### `agent/kill`

```json
Input:  { "pid": 77, "reason": "task complete" }
Output: { "killed": true }
```

### `pipe/open`

```json
Input:  { "targetPid": 59, "direction": "out", "bufferTokens": 8192, "backpressure": "block" }
Output: { "pipeId": "pipe-001", "state": "open" }
```

### `pipe/write`

```json
Input:  { "pipeId": "pipe-001", "content": "chunk of data..." }
Output: { "tokensSent": 47, "bufferRemaining": 8145 }
```

### `pipe/read`

```json
Input:  { "pipeId": "pipe-001", "maxTokens": 2048, "timeoutMs": 5000 }
Output: { "content": "...", "tokensRead": 312, "pipeState": "open" }
```

### `pipe/close`

```json
Input:  { "pipeId": "pipe-001" }
Output: { "closed": true }
```

### `cap/request-tool`

```json
Input:  { "tool": "send_email", "reason": "notify user when done", "urgency": "low" }
Output (approved): { "granted": true,  "scope": "session", "tool": "send_email" }
Output (denied):   { "granted": false, "tool": "send_email", "reason": "Use in-app notification." }
```

### `cap/escalate`

```json
Input:
{
  "reason": "I found salary data. Unsure whether to include it.",
  "context": "Researching Q3 financials. Found /data/payroll.csv.",
  "options": [
    { "id": "include", "label": "Include with PII redacted" },
    { "id": "exclude", "label": "Exclude entirely" }
  ]
}
Output: { "selectedOption": "exclude", "guidance": "Exclude salary data. Focus on revenue." }
```

### `cap/list`

```json
Input:  (none)
Output:
{
  "grantedTools": ["web_search", "fs/read", "llm/complete"],
  "constraints": {
    "maxTokensPerTurn": null,
    "maxToolChainLength": 8,
    "toolCallBudgets": { "send_email": 1 }
  },
  "tokenExpiresAt": "2026-03-21T11:00:00Z"
}
```

### `job/watch`

```json
Input:  { "jobId": "job-7f3a9b", "timeoutSec": 300 }
Output: { "jobId": "job-7f3a9b", "finalStatus": "done", "result": { ... }, "error": null }
```

### `sys/tools`

```json
Input:  { "namespace": "fs", "keyword": "", "granted_only": false }
Output:
{
  "tools": [
    { "name": "fs/read",  "description": "Read the contents of a file", "state": "available" },
    { "name": "fs/write", "description": "Write data to a file",        "state": "available" }
  ]
}
```

All three input fields are optional. Omitting them returns all registered tools.
`granted_only: true` restricts results to tools in the caller's `CapabilityToken.granted_tools`.

---

## Budget Enforcement

Each agent has a per-tool call budget stored in its `RuntimeExecutor` state:

```rust
budget: HashMap<ToolName, u32>   // remaining calls per tool
```

Before dispatching a tool call (step 4 of the turn loop), `RuntimeExecutor` atomically
decrements the budget. If the budget reaches 0, the call returns `EBUDGET` without
being dispatched, and the error is included in the message history so the LLM can
report it to the user.

Budgets are reset at the start of each agent session (not each turn). They are
persisted in the agent's snapshot (used by `agent/pause` + restore).

---

## Integration with ProcessTable

Every running agent has an entry in the `ProcessTable` under its PID. The entry
includes:

- `status`: `Running`, `Paused`, `WaitingForHuman`, `Exiting`
- `token`: the current `CapabilityToken`
- `tool_chain_depth`: current depth in the turn loop (for timeout detection)
- `budget`: reference to the per-tool budget map

`RuntimeExecutor` updates `ProcessTable` entries synchronously on state transitions.
The kernel reads these entries to implement `/proc/<pid>/status.yaml`.

---

## Open Conflicts Summary

| # | Topic | Spec says | Arch doc said | Impl today | Decision needed |
|---|-------|-----------|---------------|------------|-----------------|
| 1 | `agent/kill` | Not in spec | Present | Implemented | Add to spec or remove from impl? |
| 2 | Category 3 definition | Transparent behaviors (HIL, token renewal…) | MCP-bridged tools | MCP treated as Cat1 | Which definition? |
| 3 | Capability key format | `spawn`, `pipe` (bare) vs `llm:inference` (namespaced) | `agent:spawn`, `pipe:use` (all namespaced) | `spawn`, `pipe` (follows spec) | Standardise all to `namespace:verb`? |
| 4 | `granted_tools` content | Token schema shows individual tool names | Impl uses group names for Cat2 | Group names for Cat2, tool names for validation | One format or both? |
| 5 | Block 2 dynamism | Static at spawn | Rebuilt on `tool.changed` | Static (follows spec) | Dynamic tool list or static guidance? |
