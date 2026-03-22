# RuntimeExecutor — Tool Exposure Spec

← [Back to Schema Index](./README.md)

**Component:** `RuntimeExecutor` (inside `avix-core`)  
**Depends on:** `tool-registry.svc`, `llm.svc`, `router.svc`, `AVIX_KERNEL_SOCK`  
**Related:** LLM Service Spec, CapabilityToken, AgentManifest

-----

## Overview

The `RuntimeExecutor` is the loop that owns an agent’s entire existence: it builds the
tool list the LLM sees, runs the LLM turn, intercepts every tool call before it
executes, enforces policy, dispatches approved calls via IPC, injects results back into
the conversation, and repeats.

This document specifies exactly how Avix’s capabilities — IPC, signals, HIL, pipes,
capability upgrades, and more — are exposed to the LLM. The answer is always the same:
**as tools**. The LLM never directly touches IPC, never sees capability tokens, never
knows it is paused for HIL. It calls tools; `RuntimeExecutor` handles everything else.

-----

## The Exposure Model

Avix features fall into three categories based on how the LLM accesses them:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  Category 1 — Direct Tools                                                  │
│  The LLM calls these directly. Straightforward tool dispatch.               │
│                                                                             │
│  fs/read, fs/write, llm/complete, exec/python, mcp/github/list-prs, ...    │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│  Category 2 — Avix Behaviour Tools                                          │
│  Avix-specific capabilities exposed as tools the LLM can explicitly call.   │
│  RuntimeExecutor translates each into the correct kernel syscall or IPC.    │
│                                                                             │
│  agent/spawn, agent/kill, agent/list, agent/wait, agent/send-message       │
│  pipe/open, pipe/write, pipe/read, pipe/close                              │
│  cap/request-tool, cap/escalate                                             │
│  job/watch                                                                  │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│  Category 3 — Transparent RuntimeExecutor Behaviours                        │
│  The LLM never calls these. RuntimeExecutor handles them automatically.      │
│                                                                             │
│  HIL gating (tool_call_approval), token renewal, snapshot triggers,         │
│  tool list refresh on tool.changed, stop-reason detection                   │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│  Category 4 — MCP-Bridge Tools                                              │
│  Tools from connected MCP servers, proxied by mcp-bridge.svc.               │
│  Registered dynamically as MCP servers connect/disconnect.                  │
│                                                                             │
│  mcp/<server>/list-prs, mcp/<server>/create-issue, ...                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

-----

## Category 1 — Direct Tools

These are already covered by the tool namespace and LLM service spec. The LLM calls
them, RuntimeExecutor validates the capability token, dispatches via IPC, and injects
the result. No special handling needed beyond the standard loop.

Full namespace reference:

|Namespace      |Tools                                                       |Capability required               |
|---------------|------------------------------------------------------------|----------------------------------|
|`fs/`          |read, write, list, copy, move, watch, search                |`fs:read`, `fs:write`             |
|`llm/`         |complete, generate-image, generate-speech, transcribe, embed|`llm:inference`, `llm:image`, etc.|
|`exec/`        |runtime/python/run, runtime/shell/run, tool/git/*, pkg/uv/* |`exec:python`, `exec:shell`       |
|`mcp/<server>/`|any tool from a connected MCP server                        |per-tool grant                    |
|`jobs/`        |watch, cancel                                               |(see Category 2)                  |

-----

## Category 2 — Avix Behaviour Tools

These are the focus of this document. Each tool is a thin wrapper that RuntimeExecutor
registers on behalf of the agent at spawn time, based on what the agent’s capability
token grants. They translate LLM-callable actions into kernel syscalls or IPC calls.

They live under the `agent/`, `pipe/`, `cap/`, and `job/` namespaces in `/tools/`.
RuntimeExecutor registers only the subset the agent is entitled to — ungranted tools
are never placed in the tool list sent to the LLM.

-----

### `agent/` — Multi-Agent Orchestration

Requires the individual tools `agent/spawn`, `agent/kill`, etc. in `CapabilityToken.granted_tools`.
These are granted by expanding the `agent:spawn` capability at token issuance time.

-----

#### `agent/spawn`

Spawn a child agent to work on a sub-task. The current agent becomes the parent.
The child’s CapabilityToken is scoped to at most the parent’s grants — a spawned agent
can never exceed its parent’s permissions.

**When to use:** decomposing a large task into parallel or sequential sub-tasks, each
handled by a specialised agent.

**Input:**

```json
{
  "agent": "researcher",          // agent name — must exist in /bin/
  "goal": "Find revenue figures for Q3 2025 from SEC filings",
  "capabilities": ["web", "read"],// requested caps (subset of parent's grants)
  "waitForResult": false          // if true: blocks until child finishes (sync)
}
```

**Output:**

```json
{
  "pid": 58,
  "status": "running",            // running | completed (if waitForResult: true)
  "result": null                  // populated if waitForResult: true and child finished
}
```

**RuntimeExecutor translation:**

```
LLM calls agent/spawn
  → RuntimeExecutor validates: spawn capability granted
  → kernel/proc/spawn { agent, goal, parent_pid: self.pid, capabilities }
  → kernel returns { pid: 58, token, ipc_endpoint }
  → if waitForResult: true → kernel/proc/wait { pid: 58 }
  → return { pid: 58, status, result }
```

-----

#### `agent/kill`

Terminate a child agent by PID. The child receives `SIGKILL` and its resources are
released. Only the parent agent (or an agent with explicit kill permission) can
terminate a child.

**Input:**

```json
{
  "pid": 58,
  "reason": "Task complete — no longer needed"
}
```

**Output:**

```json
{
  "killed": true,
  "pid": 58
}
```

**RuntimeExecutor translation:** `kernel/proc/kill { pid: 58 }`

-----

#### `agent/list`

List agents currently running in this session, optionally filtered by status.

**Input:**

```json
{
  "status": "running"             // optional: running | paused | waiting | all
}
```

**Output:**

```json
{
  "agents": [
    { "pid": 58, "name": "researcher", "status": "running",
      "goal": "Find revenue figures...", "spawnedBy": 57 },
    { "pid": 59, "name": "writer",     "status": "waiting",
      "goal": "Draft the report",       "spawnedBy": 57 }
  ]
}
```

**RuntimeExecutor translation:** `kernel/proc/list { filter_type: "agent", spawned_by: self.pid }`

-----

#### `agent/wait`

Block until a specific child agent completes. The parent agent’s turn is suspended;
the LLM context is preserved. When the child finishes, the parent resumes with the
child’s result injected as the tool result.

**Input:**

```json
{
  "pid": 58,
  "timeoutSec": 300               // optional — 0 = wait forever
}
```

**Output:**

```json
{
  "pid": 58,
  "finalStatus": "completed",     // completed | stopped | crashed | timeout
  "result": "Q3 revenue was $4.2B based on EDGAR filing...",
  "durationSec": 42
}
```

**RuntimeExecutor translation:** `kernel/proc/wait { pid: 58, timeout: 300 }`

-----

#### `agent/send-message`

Send a message to another agent via its input pipe. The target agent receives the
message as user input on its next turn. Used for loose coordination without blocking.

**Input:**

```json
{
  "pid": 59,
  "message": "The research is done. Revenue figure: $4.2B. You can start drafting."
}
```

**Output:**

```json
{ "delivered": true }
```

**RuntimeExecutor translation:** `pipe/write` to the inter-agent pipe established at
spawn. If no pipe exists, RuntimeExecutor opens one automatically (requires `pipe`
capability) before delivering the message.

-----

### `pipe/` — Inter-Agent Data Channels

Requires the individual tools `pipe/open`, `pipe/write`, etc. in `CapabilityToken.granted_tools`.
These are granted by expanding the `pipe:use` capability at token issuance time.
Intra-crew pipes where
`pipePolicy: allow-intra-crew` skip the capability check.

-----

#### `pipe/open`

Open a streaming data channel to another agent. Unlike `agent/send-message` (which
sends a one-off string), pipes carry continuous token streams — useful for an agent
that generates data and wants another agent to process it incrementally.

**Input:**

```json
{
  "targetPid": 59,
  "direction": "out",             // out | bidirectional
  "bufferTokens": 8192,           // optional
  "backpressure": "block"         // block | drop | error
}
```

**Output:**

```json
{
  "pipeId": "pipe-001",
  "state": "open"
}
```

**RuntimeExecutor translation:** ResourceRequest `{ resource: pipe, targetPid: 59, ... }`
to kernel. Kernel creates the pipe record at `/proc/<pid>/pipes/pipe-001.yaml`.

-----

#### `pipe/write`

Write tokens into an open outbound pipe.

**Input:**

```json
{
  "pipeId": "pipe-001",
  "content": "Here is the next chunk of data: ..."
}
```

**Output:**

```json
{ "tokensSent": 47, "bufferRemaining": 8145 }
```

-----

#### `pipe/read`

Read tokens from an open inbound pipe. Blocks if the buffer is empty (the writing
agent hasn’t sent data yet).

**Input:**

```json
{
  "pipeId": "pipe-001",
  "maxTokens": 2048,              // optional — returns up to this many
  "timeoutMs": 5000               // optional — 0 = block indefinitely
}
```

**Output:**

```json
{
  "content": "...received data...",
  "tokensRead": 312,
  "pipeState": "open"             // open | closed (writer closed the pipe)
}
```

-----

#### `pipe/close`

Close a pipe. The other end receives `SIGPIPE`.

**Input:**

```json
{ "pipeId": "pipe-001" }
```

**Output:**

```json
{ "closed": true }
```

-----

### `cap/` — Capability Management

These tools expose the agent’s own capability state and allow it to request changes.
They map to HIL scenarios 2 and 3 from the ATP spec.

-----

#### `cap/request-tool`

Request access to a tool not currently in the agent’s CapabilityToken. This always
triggers a HIL `capability_upgrade` event — the agent suspends and waits for a human
to approve or deny. The LLM calls this when it has determined it needs a tool to
complete its goal but doesn’t currently have it.

This is **not** a direct grant mechanism. It is a request that a human must approve.
The LLM should provide a clear reason — that reason is shown verbatim to the human
in the HIL prompt.

**Input:**

```json
{
  "tool": "send_email",
  "reason": "I need to notify the user when the analysis is complete.",
  "urgency": "low"                // low | medium | high
}
```

**Output (on approval):**

```json
{
  "granted": true,
  "scope": "session",             // once | session
  "tool": "send_email"
}
```

**Output (on denial):**

```json
{
  "granted": false,
  "tool": "send_email",
  "reason": "Use the in-app notification instead."  // human's reason, if given
}
```

**RuntimeExecutor translation:**

```
LLM calls cap/request-tool { tool: "send_email", reason: "..." }
  → ResourceRequest { resource: tool, name: send_email, reason: "..." } to kernel
  → kernel opens HIL, sends SIGPAUSE to agent
  → agent loop suspends (LLM context preserved in working memory)
  → human approves / denies via ATP
  → kernel sends SIGRESUME { decision, scope?, new_capability_token? }
  → if approved: RuntimeExecutor replaces held CapabilityToken
                 tool-registry returns updated tool list on next turn
                 LLM now sees send_email in its tools — can call it directly
  → return grant result to LLM as tool result
```

After approval, the LLM doesn’t need to call `cap/request-tool` again — `send_email`
is now in its tool list and can be called directly.

-----

#### `cap/escalate`

Proactively ask a human for guidance when the agent is uncertain how to proceed. This
is open-ended — not tied to a specific tool. The agent provides a situation description
and a list of options; the human picks one and may add free-text guidance.

The agent should use this when it has detected a situation requiring human judgment
that it cannot resolve from context alone: ethical uncertainty, PII, ambiguous
instructions, destructive or irreversible actions, out-of-scope requests.

**Input:**

```json
{
  "reason": "I found salary data in the dataset. I'm unsure whether to include it.",
  "context": "Researching Q3 financials. Found /data/payroll.csv linked from the report.",
  "options": [
    { "id": "include",    "label": "Include with PII redacted" },
    { "id": "exclude",    "label": "Exclude entirely" },
    { "id": "ask_owner",  "label": "Contact the data owner first" }
  ]
}
```

**Output (on response):**

```json
{
  "selectedOption": "exclude",
  "guidance": "Yes, exclude salary data entirely. Focus on revenue and margins."
}
```

**RuntimeExecutor translation:**

```
LLM calls cap/escalate { reason, context, options }
  → SIGESCALATE to kernel with payload
  → kernel opens HIL, agent is already paused (it sent the signal)
  → human responds via ATP
  → RuntimeExecutor injects guidance as a system instruction into LLM context:
      [Human guidance]: Exclude salary data entirely. Focus on revenue and margins.
  → return { selectedOption, guidance } to LLM as tool result
  → LLM continues with human guidance now in its working context
```

No CapabilityToken change — this is purely guidance injection.

-----

#### `cap/list`

Introspect the agent’s current granted tools and constraints. Useful for the LLM to
understand what it can and cannot do before deciding whether to call `cap/request-tool`.

**Input:** (none)

**Output:**

```json
{
  "grantedTools": ["web_search", "web_fetch", "fs/read", "fs/write", "llm/complete"],
  "constraints": {
    "maxTokensPerTurn": 8000,
    "maxToolChainLength": 8,
    "toolCallBudgets": {}           // e.g. { "send_email": 1 } after scope:once grant
  },
  "tokenExpiresAt": "2026-03-21T11:00:00Z"
}
```

**RuntimeExecutor translation:** reads directly from the in-memory CapabilityToken —
no IPC call. Never exposes the token’s HMAC signature or internal structure.

-----

### `job/` — Long-Running Job Management

-----

#### `job/watch`

Subscribe to events from a long-running job. Blocks the current LLM turn and streams
job events as they arrive. When the job completes or fails, the final result is
returned as the tool result and the LLM continues.

Used when a tool returns a `job_id` instead of a direct result (tools with `job: true`
in their descriptor — e.g., `exec/runtime/python/run` with a long script, or
`llm/generate-speech` with a large audio file).

**Input:**

```json
{
  "jobId": "job-7f3a9b",
  "timeoutSec": 300
}
```

**Output:**

```json
{
  "jobId": "job-7f3a9b",
  "finalStatus": "done",          // done | failed | timeout | cancelled
  "result": { ... },              // populated on done
  "error": null                   // populated on failed
}
```

**Streaming behaviour:** while waiting, RuntimeExecutor forwards `progress` events from
`jobs.svc` as assistant content in the conversation stream so the user can see progress
in real time. These progress updates are visible to the user but are not injected into
the LLM’s conversation history — only the final result is.

-----

## Category 4 — MCP-Bridge Tools

Tools from connected MCP servers are proxied by `mcp-bridge.svc` and registered
dynamically in the tool registry as MCP servers connect or disconnect. They appear
in the `mcp/<server>/` namespace.

The LLM calls them like any other tool. RuntimeExecutor routes them to `router.svc`
which forwards to `mcp-bridge.svc`. No special handling beyond standard Cat1 dispatch.

**Tool naming:**
- Avix name: `mcp/github/list-prs` (with `/`)
- Wire name: `mcp__github__list-prs` (mangled `__` on the wire)

**Registration:** `mcp-bridge.svc` calls `ipc.tool-add` when an MCP server connects and
`ipc.tool-remove` when it disconnects. RuntimeExecutor picks up the change via
`tool.changed` events and refreshes Block 2 of the system prompt on the next turn.

**Capability:** Each MCP tool requires a per-tool grant in `CapabilityToken.granted_tools`
(e.g., `"mcp/github/list-prs"`). A wildcard grant `"mcp/github/*"` is not currently
supported — grants are always per-tool.

-----

## Category 3 — Transparent RuntimeExecutor Behaviours

These are things `RuntimeExecutor` handles automatically. The LLM never sees them,
never calls them, and is not aware they are happening.

-----

### Tool List Construction

At the start of each turn, RuntimeExecutor builds the exact tool list passed to
`llm/complete` by:

```
1. Ask tool-registry.svc for all tools where:
     - tool is in CapabilityToken.spec.tools.granted, AND
     - tool.status.state == available
2. Always include Category 2 tools the agent is entitled to:
     - Each tool in CapabilityToken.granted_tools that is a known Cat2 tool
     - cap/request-tool, cap/escalate, cap/list — always included (no token check)
     - job/watch — always included (no token check)
3. Pass the full list through the provider adapter's translate_tools()
4. Send translated list in the tools[] field of the llm/complete call
```

The LLM only ever sees the tools it is entitled to. A text-only agent with no `spawn`
capability never sees `agent/spawn` in its tool list.

-----

### HIL Tool Call Approval (Scenario 1)

When the LLM calls a tool that is in its CapabilityToken but is flagged as requiring
human approval (`hilRequiredTools` in `kernel.yaml`), RuntimeExecutor intercepts
before dispatching and suspends automatically:

```
LLM calls send_email { to: "team@org.com", subject: "Summary ready" }
  → RuntimeExecutor: tool IS in CapabilityToken ✓
  → RuntimeExecutor: send_email IS in hilRequiredTools
  → ResourceRequest { resource: tool_call_approval, tool, args } to kernel
  → kernel mints ApprovalToken, sends SIGPAUSE
  → RuntimeExecutor suspends loop (does NOT call router.svc yet)
  → human approves or denies via HIL
  → SIGRESUME received:
      approved → RuntimeExecutor dispatches the call via router.svc
               → result injected into conversation
               → LLM continues
      denied   → RuntimeExecutor injects denial as tool result:
                   { "error": "Tool call denied by human: Don't send to that address" }
               → LLM sees the denial and decides next step
```

The LLM never knows a HIL gate existed. It called a tool; it received either a result
or an error. The pause is transparent at the LLM level.

-----

### Token Renewal

CapabilityTokens have a TTL (`expiresAt`). RuntimeExecutor monitors the token expiry
and automatically renews it before it expires, without interrupting the LLM turn:

```
Token expiry - 5 minutes:
  → RuntimeExecutor sends ResourceRequest { resource: token_renewal } to kernel
  → kernel issues new token (same grants, new expiry + signature)
  → RuntimeExecutor replaces held token atomically
  → LLM turn continues uninterrupted
```

-----

### Tool List Refresh on `tool.changed`

When `llm.svc` emits a `tool.changed` event (e.g., a provider goes down, an MCP server
reconnects), RuntimeExecutor receives it via its subscription to `tool-registry.svc`
and refreshes the tool list at the next turn boundary:

```
tool.changed event: { op: "removed", tool: "mcp/github/list-prs", reason: "API unreachable" }
  → RuntimeExecutor marks tool as unavailable in local tool list cache
  → On next llm/complete call: tool is excluded from the tools[] array
  → If LLM had previously used this tool: no impact on conversation history
  → If LLM tries to call it: RuntimeExecutor rejects with a tool_result error
                              before hitting router.svc
```

-----

### Stop Reason Handling

When `llm/complete` returns a `stopReason`, RuntimeExecutor interprets it to decide
what to do next:

|`stopReason`   |RuntimeExecutor action                                                     |
|---------------|---------------------------------------------------------------------------|
|`end_turn`     |LLM is done with this turn. Return result to user, end turn.               |
|`tool_use`     |LLM called one or more tools. Dispatch each, inject results, continue loop.|
|`max_tokens`   |Context is full. Trigger memory eviction / summarisation, retry.           |
|`stop_sequence`|Agent-defined completion marker hit. Treat as `end_turn`.                  |

-----

## System Prompt Construction

On every turn, RuntimeExecutor builds the system prompt from composable blocks. The
LLM always knows what it is, what it can do, and what its current goal is.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ BLOCK 1 — Agent Identity (static, set at spawn)                             │
│                                                                             │
│ You are {agent.name}, an AI agent running inside Avix.                      │
│ Your goal: {goal}                                                           │
│ Session: {sessionId} | PID: {pid} | User: {spawnedBy}                       │
└─────────────────────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────────────────────┐
│ BLOCK 2 — Available Tools (dynamic, rebuilt on tool.changed events)         │
│                                                                             │
│ # Available Tools                                                           │
│ - **fs/read**: Read the contents of a file                                  │
│ - **agent/spawn**: Spawn a child agent to work on a sub-task                │
│ - ...                                                                       │
│ When you need a tool not listed here, call cap/request-tool.               │
│ When you encounter a situation requiring human judgment, call cap/escalate. │
│ When your task is complete, respond with your final answer.                 │
└─────────────────────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────────────────────┐
│ BLOCK 3 — Constraints (static, set at spawn)                                │
│                                                                             │
│ Max tool calls per turn: {maxToolChainLength}                               │
│ Context limit: {contextLimit} tokens                                        │
│ [If toolCallBudgets non-empty]: send_email: 1 use remaining                 │
└─────────────────────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────────────────────┐
│ BLOCK 4 — Pending Instructions (dynamic, injected by RuntimeExecutor)       │
│                                                                             │
│ Populated when events occur mid-session:                                    │
│  - HIL escalation guidance: "[Human guidance]: Exclude salary data."        │
│  - HIL denial feedback: "[Human]: Don't send to that address."              │
│  - Tool availability change: "[System]: mcp/github is currently unavailable"│
│  - Memory summary: "[Context summary]: Earlier you found..."                │
└─────────────────────────────────────────────────────────────────────────────┘
```

Blocks 1 and 3 are static — fixed at spawn and never change. Block 2 is rebuilt
whenever a `tool.changed` event fires so the LLM always sees its current tool set.
Block 4 is populated at runtime as events occur.

-----

## Complete Turn Loop

Putting it all together, the full loop `RuntimeExecutor` runs on every turn:

```
START OF TURN
│
├── 1. Refresh tool list (if tool.changed received since last turn)
│
├── 2. Build system prompt blocks 1–4
│
├── 3. Call llm/complete {
│         model, messages, tools: [translated descriptors],
│         system: <assembled blocks>, maxTokens, temperature
│       }
│
├── 4. Receive response
│      ├── stopReason == end_turn  → return result to user → END
│      ├── stopReason == max_tokens → evict/summarise context → GOTO 2
│      └── stopReason == tool_use  → CONTINUE
│
├── 5. For each tool call in response:
│      │
│      ├── a. Validate: tool in CapabilityToken?
│      │     NO  → inject error result: "Tool not granted: <name>"
│      │     YES → continue
│      │
│      ├── b. Check toolCallBudget (scope:once grants)
│      │     EXCEEDED → inject error result: "Tool call budget exhausted"
│      │     OK       → continue
│      │
│      ├── c. HIL approval check: tool in hilRequiredTools?
│      │     YES → ResourceRequest tool_call_approval to kernel
│      │           → suspend loop (SIGPAUSE)
│      │           → await SIGRESUME
│      │           → denied: inject denial as tool result → continue loop
│      │           → approved: continue to (d)
│      │     NO  → continue to (d)
│      │
│      ├── d. Dispatch via router.svc → target service
│      │
│      └── e. Inject tool result into messages
│               Category 2 tool (agent/spawn, cap/request-tool, etc.):
│                 RuntimeExecutor handles internally before dispatching
│               Category 1 tool (fs/read, etc.):
│                 Pass-through to service, inject result directly
│
├── 6. Append all tool results to messages
│
└── 7. GOTO 1 (next turn)
```

-----

## Capability → Tool Mapping Reference

This table shows which Category 2 tools become available for each capability grant.
RuntimeExecutor uses this to build the per-agent tool list at spawn time.

This table maps capability names (used by token issuers) to the individual tool names
stored in `CapabilityToken.granted_tools`. RuntimeExecutor checks for each tool name
individually — it never checks for capability group names.

|Capability key     |Individual tools granted                                                           |
|-------------------|-----------------------------------------------------------------------------------|
|`agent:spawn`      |`agent/spawn`, `agent/kill`, `agent/list`, `agent/wait`, `agent/send-message`      |
|`pipe:use`         |`pipe/open`, `pipe/write`, `pipe/read`, `pipe/close`                               |
|`llm:inference`    |`llm/complete`                                                                     |
|`llm:image`        |`llm/generate-image`                                                               |
|`llm:speech`       |`llm/generate-speech`                                                              |
|`llm:transcription`|`llm/transcribe`                                                                   |
|`llm:embedding`    |`llm/embed`                                                                        |
|*(always)*         |`cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch` — no token check       |

`cap/request-tool`, `cap/escalate`, and `cap/list` are always in the tool list
regardless of grants — an agent always needs the ability to ask for more tools or
escalate to a human. `job/watch` is always available because any tool with `job: true`
can run, and the LLM needs a way to await the result.

-----

## Tool Descriptor Registration

Category 2 tools are registered by `RuntimeExecutor` itself via `ipc.tool-add` at
agent spawn, not statically via `.tool.yaml` files. They are registered with
`visibility: user:<spawnedBy>` so they are scoped to this agent’s session and do not
appear in any other agent’s tool list.

At agent exit, RuntimeExecutor calls `ipc.tool-remove` for all Category 2 tools it
registered. This ensures clean removal from the registry with no orphaned descriptors.

-----

## Related Documents

- [LLM Service Spec](./llm-service.md) — `llm/complete` and provider adapters
- [ATP Spec](./atp.md) — HIL event protocol (hil.request, SIGRESUME)
- [CapabilityToken](./capability-token.md) — token structure and lifecycle
- [Pipe](./pipe.md) — pipe resource schema
- [Signal](./signal.md) — SIGPAUSE, SIGRESUME, SIGESCALATE
- [ResourceRequest](./resource-request.md) — kernel request protocol
- [AgentManifest](./agent-manifest.md) — capability declarations at install time
