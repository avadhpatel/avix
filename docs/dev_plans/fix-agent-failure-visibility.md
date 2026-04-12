# Dev Plan: Agent Failure Partial Output Visibility

## Problem

When an agent fails (e.g. max tool calls reached), the UI shows only "agent failed"
with no conversation history, no tool calls, and no LLM output visible. The user has
no way to inspect what the agent did before it failed.

## Root Cause Analysis

Two independent gaps combine to produce this symptom:

### Gap 1 — `AgentToolCall`/`AgentToolResult` dropped in event bridge

`crates/avix-client-core/src/state.rs` → `start_event_bridge()` (line 168):

```rust
let event_name: &str = match event.kind {
    EventKind::SessionReady    => "daemon-ready",
    EventKind::AgentSpawned    => "agent.spawned",
    EventKind::AgentOutput     => "agent.output",
    EventKind::AgentOutputChunk => "agent.output.chunk",
    EventKind::AgentStatus     => "agent.status",
    EventKind::AgentExit       => "agent.exit",
    EventKind::ToolChanged     => "tool.changed",
    EventKind::SysService      => "sys.service",
    _ => continue,   // ← AgentToolCall + AgentToolResult silently dropped
};
```

These events are published by `dispatch_manager.rs` for every tool call and result,
they reach the ATP bus, pass through gateway → dispatcher, but are never forwarded
to the UI. The browser therefore has zero real-time visibility into tool execution.

**Event body shapes (from `event_bus.rs`):**

| Event | Body fields |
|-------|-------------|
| `AgentToolCall` | `{ pid, callId, tool, args }` |
| `AgentToolResult` | `{ pid, callId, tool, result }` |

### Gap 2 — `SessionPage` never reloads conversation on `agent.exit`

`crates/avix-app/src/src/pages/SessionPage.tsx` calls `loadMessages()` exactly once:
in the `useEffect` that runs when `selectedSessionId` changes (line 138–144).

When an agent exits (success or failure), `executor_factory.rs` calls
`shutdown_with_status()` which writes the full conversation to disk, then emits
`agent_exit`. The conversation is on disk — but `SessionPage` never re-fetches it
because no code path calls `loadMessages()` in response to `agent.exit`.

`AppContext.tsx` does handle `agent.exit` (line 170–173): it calls `removeAgent`
and `refreshSessions()`. This updates the session status badge in the UI, but does
not trigger a message reload in `SessionPage`.

## Confirmed Working (no change needed)

- ATP event bus publishes `AgentToolCall`/`AgentToolResult` correctly.
- Gateway WebSocket server forwards all filter-passing events.
- Browser `web.ts` `listen()` dispatches by event name correctly.
- `shutdown_with_status()` writes conversation before emitting `agent.exit`.
- `agent.exit` reaches browser (gap is only in bridge + UI refresh).

## Fix Plan

### File 1: `crates/avix-client-core/src/state.rs`

Add the two missing event kinds to the `start_event_bridge` match:

```rust
EventKind::AgentToolCall    => "agent.tool_call",
EventKind::AgentToolResult  => "agent.tool_result",
```

No other changes to this file.

### File 2: `crates/avix-app/src/src/context/AppContext.tsx`

After the `agent.exit` listener fires, also trigger a conversation reload for
`SessionPage`. The cleanest mechanism: expose a `lastExitPid` counter (or a
`conversationRefreshKey` counter) from context so `SessionPage` can `useEffect`
on it without coupling contexts directly.

Add to context state:
```ts
const [conversationVersion, setConversationVersion] = useState(0);
```

In the `agent.exit` handler:
```ts
listen<{ pid: number; exitCode?: number }>('agent.exit', (e) => {
  removeAgent(e.payload.pid);
  refreshSessions();
  setConversationVersion((v) => v + 1);   // ← triggers SessionPage reload
}),
```

Expose `conversationVersion` in context value and `AppContextValue` interface.

Also add listeners for `agent.tool_call` and `agent.tool_result` to accumulate live
tool activity into a per-pid ring buffer (last N entries) for display. Add to context:

```ts
// Maps pid → array of {callId, tool, args?, result?, isResult: bool}
const [liveToolCalls, setLiveToolCalls] = useState<Record<number, LiveToolEntry[]>>({});
```

Expose `liveToolCalls` in context.

### File 3: `crates/avix-app/src/src/types/agents.ts`

Add the `LiveToolEntry` type:
```ts
export interface LiveToolEntry {
  callId: string;
  tool: string;
  args?: unknown;
  result?: string;
  isResult: boolean;
  timestamp: number;
}
```

### File 4: `crates/avix-app/src/src/pages/SessionPage.tsx`

1. Subscribe to `conversationVersion` from context. Add it as a dependency to the
   messages-reload `useEffect`:
   ```ts
   const { ..., conversationVersion } = useApp();
   useEffect(() => {
     setSession(null);
     setInvocationMessages([]);
     setInputText('');
     loadSession();
     loadMessages();
   }, [selectedSessionId, conversationVersion, loadSession, loadMessages]);
   ```
   This ensures `loadMessages()` fires whenever any agent in this session exits.

2. Display live tool calls from context: in the streaming section, below the live
   text block, render the `liveToolCalls[activePid]` ring (if any) as a compact
   tool activity feed:
   ```tsx
   {liveToolCalls[activePid]?.length > 0 && (
     <div style={{ marginBottom: 16 }}>
       {liveToolCalls[activePid].slice(-10).map((tc) => (
         <div key={tc.callId + tc.isResult} style={{ fontSize: 11, color: tc.isResult ? '#a6e3a1' : '#f9e2af', ... }}>
           {tc.isResult ? '← ' : '→ '}{tc.tool}
           {tc.result && `: ${tc.result.slice(0, 80)}`}
         </div>
       ))}
     </div>
   )}
   ```

3. Clear `liveToolCalls` for a pid on `agent.exit` (handle in AppContext).

## Implementation Order

1. `crates/avix-client-core/src/state.rs` — add 2 event kind arms (backend fix)
2. `crates/avix-app/src/src/types/agents.ts` — add `LiveToolEntry` type
3. `crates/avix-app/src/src/context/AppContext.tsx` — add `conversationVersion`,
   `liveToolCalls`, and new event listeners
4. `crates/avix-app/src/src/pages/SessionPage.tsx` — use `conversationVersion` +
   render live tool feed

## Testing Strategy

### Backend (Rust)
- `cargo test --package avix-client-core` — confirm no regressions
- Manually spawn an agent, observe server logs confirm `agent.tool_call` events
  are now emitted to the bridge (not just dropped)

### Frontend (manual)
- Spawn an agent that hits max tool calls
- Verify: live tool calls appear during execution
- Verify: after `agent.exit`, conversation history is loaded automatically (no manual
  refresh needed)
- Verify: "agent failed — read-only" input bar is shown with the full history visible

## Files Changed

| File | Change |
|------|--------|
| `crates/avix-client-core/src/state.rs` | Add 2 event kinds to bridge match |
| `crates/avix-app/src/src/types/agents.ts` | Add `LiveToolEntry` type |
| `crates/avix-app/src/src/context/AppContext.tsx` | `conversationVersion`, `liveToolCalls`, new listeners |
| `crates/avix-app/src/src/pages/SessionPage.tsx` | React to `conversationVersion`, render live tool feed |
