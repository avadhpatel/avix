# 09 — RuntimeExecutor Tool Exposure Model

## Overview

`RuntimeExecutor` mediates between the LLM (stateless — it knows only tool names and
schemas) and the Avix system. It is the stateful component that:

- Owns the agent's message history and turn budget
- Controls which tools the LLM can see and call
- Validates every tool call against the agent's `CapabilityToken`
- Enforces per-tool budgets and the maximum tool-chain length
- Handles Human-in-the-Loop (HIL) escalation (3 scenarios)
- Registers and deregisters Category 2 tools at spawn/exit

`RuntimeExecutor` never calls provider APIs directly — all AI calls go through
`llm.svc` via IPC (ADR-02).

---

## Three Tool Categories

### Category 1: Direct Service Tools

Registered by services (e.g., `fs/read`, `fs/write` by `fs.svc`; `llm/complete` by
`llm.svc`). The LLM can call them if the agent's `CapabilityToken` grants access.

`RuntimeExecutor` forwards the call to `router.svc`, which dispatches it to the owning
service. The agent's PID is injected into the request as `_caller` (enforced by
`router.svc` — the caller cannot spoof this field).

Category 1 tools have `ToolVisibility::All` unless the owning service declares otherwise.

### Category 2: Avix Behaviour Tools

Control the agent's own runtime state. These tools are **not** hard-coded in any
service's tool list. Instead, `RuntimeExecutor` registers them via `ipc.tool-add` at
agent spawn time and removes them via `ipc.tool-remove` at exit (ADR-04).

The set of Category 2 tools granted to an agent depends on its `CapabilityToken`
(see Capability-to-Tool Mapping below).

#### Full Category 2 tool list

| Tool               | Capability scope | Description                                     |
|--------------------|------------------|-------------------------------------------------|
| `agent/spawn`      | `agent:spawn`    | Spawn a child agent with a given system prompt  |
| `agent/kill`       | `agent:spawn`    | Terminate a child agent by PID                  |
| `pipe/open`        | `pipe:use`       | Open a bidirectional IPC pipe to another agent  |
| `pipe/write`       | `pipe:use`       | Write a message to an open pipe                 |
| `pipe/read`        | `pipe:use`       | Read the next message from an open pipe         |
| `pipe/close`       | `pipe:use`       | Close an open pipe                              |
| `cap/request-tool` | *(always)*       | Request a capability expansion (triggers HIL)   |
| `cap/escalate`     | *(always)*       | Escalate a decision to a human approver         |
| `cap/list`         | *(always)*       | List all currently granted capabilities         |
| `job/watch`        | *(always)*       | Subscribe to progress events for a job          |

The four always-present tools (`cap/request-tool`, `cap/escalate`, `cap/list`,
`job/watch`) are registered regardless of the agent's capability grants (Architecture
Invariant 13).

### Category 3: Transparent (MCP-Bridged) Tools

External tools bridged from MCP servers by `mcp-bridge.svc`
(e.g., `mcp/github/create-issue`, `mcp/jira/get-ticket`). Registered by
`mcp-bridge.svc` and visible to `RuntimeExecutor` via the tool registry.

`RuntimeExecutor` treats Category 3 tools identically to Category 1. The `mcp/`
namespace prefix is transparent — policy enforcement (capability check, budget
decrement) is identical regardless of category.

---

## Capability-to-Tool Mapping

The `CapabilityToolMap` is a compile-time constant inside `RuntimeExecutor`:

```
agent:spawn  → agent/spawn, agent/kill
pipe:use     → pipe/open, pipe/write, pipe/read, pipe/close
job:watch    → job/watch
(always)     → cap/request-tool, cap/escalate, cap/list, job/watch
```

At spawn, `RuntimeExecutor` iterates the agent's `CapabilityToken.granted_tools` and
the `CapabilityToolMap` to compute the final Category 2 tool set for this agent.

---

## Category 2 Registration Lifecycle

```
1. RuntimeExecutor::spawn_with_registry(token, registry)
   │
   ├─ compute Category 2 set from token + CapabilityToolMap
   │
   ├─ for each tool in set:
   │    registry.add(tool, schema, Category2, ToolVisibility::User(username))
   │    ipc.tool-add → router.svc
   │
   └─ LLM system prompt built; first turn begins

2. (agent running — tool calls dispatched normally)

3. RuntimeExecutor::shutdown()
   │
   ├─ for each registered Category 2 tool:
   │    registry.remove(tool, drain: true)   ← waits for in-flight calls
   │    ipc.tool-remove → router.svc
   │
   └─ process table entry cleared
```

**`drain: true`** ensures that any in-flight call to a Category 2 tool completes before
the tool is removed from the registry. This prevents the race condition where a tool
call is dispatched just before shutdown begins.

---

## Tool Visibility

`ToolVisibility::User(username)` scopes a Category 2 tool to the agent's owning user.
Other users' agents cannot discover or call it via `router.svc`.

Category 1 tools use `ToolVisibility::All` unless the registering service explicitly
restricts them. Category 3 (MCP) tools inherit the visibility declared by
`mcp-bridge.svc` at registration time (typically `ToolVisibility::All`).

---

## System Prompt Block Construction

Before the first turn, `RuntimeExecutor` builds the system prompt from four blocks:

### Block 1 — Identity

```
You are <agent_name> (PID <pid>).
Role: <role>
User: <username>
Crews: <crew1>, <crew2>
```

### Block 2 — Capabilities

A human-readable list of all tools the agent is currently granted, with their
descriptions and parameter schemas. This block is **rebuilt** whenever a `tool.changed`
event fires (e.g., a provider goes unhealthy), ensuring the LLM never attempts to call
a tool that is currently unavailable.

### Block 3 — Constraints

```
Max tool chain length: <N>
Budget per tool: <map of tool → remaining calls>
Content policy: <policy text>
Output format: <requirements>
```

### Block 4 — Context Injection

Used during HIL escalation. When a human approves or rejects an `cap/escalate` request,
the human's decision and any accompanying message are injected here before the turn
resumes. This is the only mechanism by which human input enters the agent's context
mid-turn.

---

## The 7-Step Turn Loop

```
┌─────────────────────────────────────────────────┐
│  1. Build tool list                             │
│     Category 1 (filtered by capability)         │
│   + Category 2 (registered at spawn)            │
│   + Category 3 (MCP-bridged, filtered)          │
│     → exclude tools flagged unhealthy by        │
│       tool.changed events                        │
│                                                  │
│  2. Call llm/complete via IPC                   │
│     messages = full history + system prompt      │
│     tools    = tool list from step 1             │
│                                                  │
│  3. Interpret stop_reason                       │
│     "end_turn"   → return result to caller       │
│     "tool_use"   → proceed to step 4            │
│     "max_tokens" → summarise history, loop back  │
│                                                  │
│  4. Validate each tool call                     │
│     token.has_tool(name)? → else EPERM          │
│     budget[name] > 0?     → else EBUDGET        │
│                                                  │
│  5. Dispatch tool calls                         │
│     Parallel where safe (no data dependencies)  │
│     Sequential when the LLM orders them so      │
│     Category 2: handled locally by RuntimeExecutor│
│     Category 1/3: forwarded to router.svc       │
│                                                  │
│  6. Append tool results to message history      │
│     Each result: { tool_use_id, content, error? }│
│                                                  │
│  7. Loop back to step 1                         │
│     Until stop_reason = "end_turn"              │
│     or tool_chain_length >= max_tool_chain_length│
└─────────────────────────────────────────────────┘
```

---

## Human-in-the-Loop (HIL) — Three Scenarios

### Scenario 1: Capability Expansion Request

The LLM calls `cap/request-tool` to request access to a tool it does not currently hold.
`RuntimeExecutor` suspends the turn (emits `SIGPAUSE`), routes the request to the
configured `human_channel`, and waits for an `ApprovalToken`.

- **Approved**: the capability is added to the agent's token; Block 2 is rebuilt; the
  turn resumes from step 1.
- **Rejected**: the rejection reason is injected via Block 4; the turn resumes with the
  tool absent from the list.

### Scenario 2: Explicit Escalation

The LLM calls `cap/escalate` to ask a human to make a decision. `RuntimeExecutor`
suspends, delivers the escalation to all `human_channel` tools simultaneously. The
first valid human response atomically invalidates all others (ADR-07 — `ApprovalToken`
is single-use).

The human's response (approve / reject + message) is injected into Block 4. The turn
resumes.

### Scenario 3: Policy Violation Intercept

If a tool call would violate the agent's content policy (e.g., a file write to a path
outside the agent's granted tree), `RuntimeExecutor` intercepts before dispatch and
emits an automatic escalation event. The human can override or the call is denied.

---

## Tool Name Mangling in Practice

`RuntimeExecutor` always works with **unmangled** Avix tool names:

1. `RuntimeExecutor` calls `llm/complete` with tool schemas using Avix names (`fs/read`).
2. `llm.svc` mangles outbound: `fs/read` → `fs__read` (sent to provider API).
3. Provider response contains `fs__read` in `tool_use` blocks.
4. `llm.svc` unmangled inbound: `fs__read` → `fs/read` (returned to RuntimeExecutor).
5. `RuntimeExecutor` dispatches `fs/read` to `router.svc` — always unmangled.

The mangling/unmangling boundary is strictly inside `llm.svc`. No other component
in Avix touches `__` tool names.

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
The kernel reads these entries to implement `/proc/<pid>/status.yaml` (always reflects
live state, never stale).
