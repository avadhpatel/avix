# Dev Plan: Streaming Events Gap D — ATP Session ID Routing

## Task Summary — ✅ COMPLETE (commit `ef603f8`, 2026-04-17)

All owner-scoped ATP events (agent output chunks, status, exit, tool calls, spawned) were
silently dropped by the gateway's ownership gate because `IpcExecutorFactory` called the
event bus with the **agent session ID** (a logical conversation UUID), but the ownership
gate compares against the **ATP connection session ID** (from the login JWT).

**Fixed**: `atp_session_id` is now threaded from `ValidatedCmd.caller_session_id` through
`SpawnParams.atp_session_id` into `IpcExecutorFactory` and `RuntimeExecutor`. All
`event_bus.*` calls use it instead of the agent session UUID. `agent.spawned` is now
emitted at executor task start with `{ pid, name, goal, sessionId }`.

Note: `agent.tool_call` and `agent.tool_result` events are **not yet emitted** — that is
a separate follow-on gap (see `streaming-events-tool-calls.md` when written).

---

## Architecture Spec References

- `docs/architecture/04-atp.md` — ownership gate: `conn.session_id == event.owner_session`
- `docs/architecture/13-streaming.md` — streaming pipeline
- `docs/architecture/07-services.md` — service lifecycle

---

## The Two Session ID Concepts

| Name | Description | Where it lives |
|------|-------------|----------------|
| **ATP connection session ID** | From `login_resp.session_id`. Identifies one WebSocket connection. Used by the ownership gate. | `ValidatedCmd.caller_session_id`, `Dispatcher.inner.session_id`, `ConnectionStatus::Connected.session_id` |
| **Agent session ID** | A logical conversation UUID. Groups one or more agent invocations. Shown in `SessionPage.tsx`. | `SpawnParams.session_id`, `InvocationRecord.session_id`, `active_sessions[pid]` |

---

## Root Cause

`IpcExecutorFactory::launch` (line 97) captures `params.session_id` as `session_id` and
passes it to every `event_bus.*` call:

```rust
// executor_factory.rs — BUG: session_id is the agent session UUID, not the ATP session
let session_id = params.session_id.clone();
// ...
event_bus.agent_status(&session_id, pid, "running");    // ← wrong
event_bus.agent_output_chunk(&session_id, pid, ...);    // ← wrong
event_bus.agent_exit(&session_id, pid, ...);            // ← wrong
```

The event bus sets `event.owner_session = agent_session_uuid`. The gateway ownership gate
then rejects every event:
```
conn.session_id ("abc-login-session") ≠ event.owner_session ("def-agent-session") → DROP
```

The ATP connection session ID (`cmd.caller_session_id`) IS available at the gateway handler
level in `ValidatedCmd` but is never injected into the spawn body or threaded to the
executor factory.

Additionally, `agent.spawned` is never published in the production `IpcExecutorFactory`
(only in test stubs in `gateway/handlers/mod.rs`).

---

## Fix: Thread ATP Session ID from Gateway to Event Bus

The ATP session ID must flow from the gateway `proc/spawn` handler down through:

```
ValidatedCmd.caller_session_id
  → spawn body["atp_session_id"]
  → IPC kernel/proc/spawn body
  → AgentManager::spawn(atp_session_id)
  → SpawnParams.atp_session_id
  → IpcExecutorFactory::launch → event_bus.*(&atp_session_id, ...)
```

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/gateway/handlers/proc.rs` | Inject `atp_session_id` from `cmd.caller_session_id` into spawn body |
| 2 | `crates/avix-core/src/executor/spawn.rs` | Add `atp_session_id: String` to `SpawnParams` |
| 3 | `crates/avix-core/src/kernel/proc/agent.rs` | Accept `atp_session_id` from spawn body; pass into `SpawnParams` |
| 4 | `crates/avix-core/src/bootstrap/executor_factory.rs` | Use `params.atp_session_id` for all `event_bus.*` calls; emit `agent.spawned` |

---

## Implementation Order

### Step 1 — `proc.rs`: Inject ATP session ID into spawn body

In `gateway/handlers/proc.rs`, the `"spawn"` arm of the `handle()` function currently
injects `caller` and `session_id` (empty default). Also inject the ATP connection session ID:

```rust
// Before:
body.as_object_mut()
    .unwrap()
    .entry("session_id")
    .or_insert(serde_json::json!(""));

// After:
body.as_object_mut()
    .unwrap()
    .entry("session_id")
    .or_insert(serde_json::json!(""));
// Inject the ATP connection session ID so the executor factory can route events correctly.
body["atp_session_id"] = serde_json::json!(cmd.caller_session_id);
```

**Test**: update `spawn_injects_caller_and_empty_session_id` test to also assert
`params["atp_session_id"] == "sess-proc"`.

Compile check: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- gateway::handlers::proc`

---

### Step 2 — `spawn.rs`: Add `atp_session_id` to `SpawnParams`

```rust
pub struct SpawnParams {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub token: CapabilityToken,
    pub session_id: String,          // agent logical session UUID
    pub atp_session_id: String,      // ATP connection session ID — for event routing
    // ... rest unchanged
}
```

Fix all `SpawnParams { .. }` construction sites that will fail to compile (add
`atp_session_id: String::new()` as a placeholder until Step 3 fills in the real value).

Compile check: `cargo check --package avix-core`

---

### Step 3 — `agent.rs`: Thread `atp_session_id` through `AgentManager::spawn`

`AgentManager::spawn` currently receives `session_id` (the logical agent session parameter)
from the IPC body. It must also extract `atp_session_id` and pass it to `SpawnParams`.

Change the IPC handler that calls `AgentManager::spawn` to extract `atp_session_id` from
the body and pass it as a parameter (or thread it via `SpawnParams` directly).

The kernel IPC server (`kernel/ipc_server.rs`) is where `kernel/proc/spawn` is handled.
Find where it calls `agent_manager.spawn(...)`, extract `body["atp_session_id"]`, and
pass it into `SpawnParams.atp_session_id`.

```rust
// In the kernel/proc/spawn IPC handler:
let atp_session_id = body["atp_session_id"].as_str().unwrap_or("").to_string();
// ...
let spawn_params = SpawnParams {
    // ...
    session_id: effective_session_id.clone(),
    atp_session_id,
    // ...
};
```

Compile check: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- kernel::proc`

---

### Step 4 — `executor_factory.rs`: Use `atp_session_id` for event bus calls + emit `agent.spawned`

Replace all `&session_id` arguments to `event_bus.*` with `&atp_session_id`:

```rust
// Before:
let session_id = params.session_id.clone();  // ← agent session UUID, WRONG for event routing

// After:
let agent_session_id = params.session_id.clone();  // kept for logging/invocation records
let atp_session_id = params.atp_session_id.clone();  // ← ATP connection session, CORRECT

// ...
event_bus.agent_status(&atp_session_id, pid.as_u64(), "running");    // ✓
event_bus.agent_status(&atp_session_id, pid.as_u64(), "crashed");    // ✓
event_bus.agent_status(&atp_session_id, pid.as_u64(), "waiting");    // ✓
event_bus.agent_exit(&atp_session_id, pid.as_u64(), exit_code);      // ✓
```

Also emit `agent.spawned` at the top of the launched task (after executor is ready):

```rust
// Emit agent.spawned so UI can register the new agent immediately.
event_bus.publish(
    AtpEvent::new(
        AtpEventKind::AgentSpawned,
        &atp_session_id,
        serde_json::json!({
            "pid": pid.as_u64().to_string(),
            "name": agent_name,
            "goal": goal,
            "sessionId": agent_session_id,   // logical session UUID — UI needs this for routing
        }),
    ),
    Some(atp_session_id.clone()),  // owner_session = atp_session_id
    Role::User,
);
```

Note: the `agent.spawned` body includes `sessionId` (the logical agent session UUID) so
the frontend can associate the agent's PID with its conversation session. This is the key
field that lets `SessionPage.tsx` know which session a newly spawned agent belongs to.

Also fix the `run_turn_streaming` path — `RuntimeExecutor` calls `event_bus.agent_output_chunk`
with `self.session_id`. Verify whether `RuntimeExecutor.session_id` is the ATP session ID or
the agent session ID; if it's the agent session ID, it needs to also carry `atp_session_id`
and use that instead for event bus calls.

Check: `crates/avix-core/src/executor/runtime_executor.rs` field `session_id` — trace what
value is assigned at `RuntimeExecutor::spawn_with_registry` from `SpawnParams`.

Compile check: `cargo check --package avix-core`
Test: `cargo test --package avix-core -- bootstrap::executor_factory`

---

## `agent.spawned` Body Design

The `agent.spawned` event body must include both IDs so the frontend can:
1. Register the new agent (pid, name)
2. Associate it with its conversation session (sessionId = logical agent session UUID)

```json
{
  "pid": "42",
  "name": "universal-tool-explorer",
  "goal": "explore tools",
  "sessionId": "def456-agent-session-uuid"
}
```

Update `AgentSpawnedBody` in `crates/avix-client-core/src/atp/types.rs` to match:

```rust
pub struct AgentSpawnedBody {
    pub pid: String,
    pub name: String,
    pub goal: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,  // logical agent session UUID
}
```

The `Event.owner_session` field (the outer ATP wire field `sessionId`) continues to carry
the ATP connection session ID — this is what the ownership gate uses. The `sessionId` inside
the body is the logical agent session, used by the frontend for navigation.

---

## Verification After Implementation

End-to-end smoke test:
1. Login → ATP connection session_id = `S_atp`
2. Spawn agent → `agent.spawned` arrives at client with `owner_session = S_atp` ✓
3. Agent runs → `agent.output.chunk` events arrive with `owner_session = S_atp` ✓
4. Agent exits → `agent.exit` arrives with `owner_session = S_atp` ✓
5. Second user logs in with `S_atp2` → their agent events don't appear in first client ✓

Targeted tests:
```bash
cargo test --package avix-core -- gateway::handlers::proc
cargo test --package avix-core -- kernel::proc
cargo test --package avix-core -- bootstrap::executor_factory
cargo test --package avix-client-core -- atp::types
```

---

## Success Criteria

- [x] `agent.output.chunk` events arrive at the UI as an agent executes (`ef603f8`)
- [x] `agent.spawned` is emitted with both `pid` and `sessionId` (logical session UUID) (`ef603f8`)
- [x] `agent.status` and `agent.exit` events arrive correctly (`ef603f8`)
- [x] `agent.tool_call` and `agent.tool_result` events arrive correctly (were emitted but used wrong session_id — fixed in `dispatch_manager.rs` alongside gap-D)
- [x] A second connected user does not receive the first user's agent events (`ef603f8`)
- [x] `AgentSpawnedBody` includes `session_id` field and frontend maps pid → session correctly (`ef603f8`)
