# Dev Plan: Session-Centric Web UI

> **Mode 1 plan** — do not begin implementation until user explicitly approves and
> switches to Mode 2.

---

## Task Summary

Redesign the avix-web sidebar and main panel around **sessions** rather than agents.
When the user connects (or reconnects after a disconnect), the UI fetches the active session
list and shows it in the sidebar. Clicking any session loads the full message thread for that
session in the main panel — with one message-bubble per turn, labelled with the agent that
generated it. If a HIL request is pending on any agent in the session, the text-input bar
is replaced with the HIL approval widget. If multiple agents are active in the session,
their status and metadata are visible in a collapsible info rail.

The **only** way to create a new session is to spawn an installed agent: the user picks an
agent from the installed-agent catalog, optionally overrides the default goal, and confirms.
This spawns a new PID which the kernel automatically links to a new session. The UI then
auto-navigates to that session.

---

## Architecture Specs Referenced

- `docs/architecture/14-agent-persistence.md` — sessions, invocations, conversation storage
- `docs/architecture/04-atp.md` — ATP events, session/proc ops, HIL flow
- `docs/architecture/12-avix-clients.md` — client-core command layer

---

## Confirmed Feature Set

1. **Session list sidebar** — replaces the current agent list; shows Running, Idle, and
   Paused sessions for the authenticated user; refreshes on `agent.spawned`/`agent.exit`
   events; each item shows title/goal preview, status badge, last_updated, and a HIL count
   badge.

2. **Reconnect load** — on connect (first load or after reconnect) the UI calls
   `list_sessions` to populate the sidebar without relying on live events.

3. **Session message thread** — clicking a session in the sidebar opens `SessionPage`,
   which fetches all invocations for that session, reads the persisted `conversation.jsonl`
   for each completed invocation, and stitches them into a single chronological message
   list. Each message-bubble shows the **agent name** as a label.

4. **Live streaming integration** — `agent.output.chunk` events are still used for the
   currently-running invocation. The historical messages from completed invocations are
   prepended ahead of the live stream.

5. **Input bar → context-aware**:
   - Session `Idle`: text field → calls `resume_session(session_id, input)`
   - Session `Running`: text field → calls `pipe_text(pid, input)` to active agent PID
   - Session `Paused` + pending HIL: renders `HilInlineCard` instead of text field
   - Session `Completed`/`Failed`: no input bar (read-only thread)

6. **Multi-agent info rail** — when `session.participants.length > 1`, a collapsible
   panel on the right side of `SessionPage` lists each participant with its last-known
   status and token count (from its `InvocationRecord`).

7. **`identity` propagation** — `AppContext` loads the authenticated user's identity from
   `auth_status` once at mount and exposes it; `list_sessions` and other username-scoped
   calls use it instead of the hardcoded `""` fallback that the gateway already handles.

8. **New Session flow** — a "New Session" button at the top of the sidebar opens a
   two-step `NewSessionModal`:
   - **Step 1 — Agent picker**: shows all installed agents (calls `list_installed`); each
     row shows agent name, version, scope badge, and description; a search field filters
     the list; clicking a row advances to step 2.
   - **Step 2 — Goal input**: shows the selected agent's name and description; a textarea
     pre-filled with `agent.description` lets the user type/override the goal; "Start"
     button calls `spawn_agent({ name: agent.name, description: goal })`.
   - After spawn: the returned PID is used to find the newly created session (scan the
     refreshed session list for `session.ownerPid === pid`); the UI auto-navigates to that
     session and closes the modal.
   - The existing `AddAgentModal` and the topbar "Add Agent" button are **removed** — this
     modal is the single entry point for starting a session.

---

## Current State Gap Analysis

| What's needed | Current state |
|---|---|
| `list_for_session(session_id)` in `InvocationStore` | Missing — only `list_for_user` / `list_for_agent` exist |
| `read_conversation(inv_id)` in `InvocationStore` | Missing — JSONL is written but never read back |
| `kernel/proc/invocation-conversation` IPC handler | Missing |
| Gateway op `"invocation-conversation"` | Missing |
| `list_sessions`, `get_session`, `resume_session` in `avix-client-core/commands.rs` | Missing |
| `get_invocation_conversation` command | Missing |
| Web routes: `list_sessions`, `get_session`, `resume_session`, `get_session_messages` | Missing |
| TypeScript `Session` / `ConversationEntry` types | Missing |
| `identity` in `AppContext` | Missing — `App.tsx` fetches it but discards it |
| Sidebar driven by sessions | Not built — shows raw agent list |
| `SessionPage` component | Not built |
| `NewSessionModal` (agent picker → goal → spawn → navigate) | Not built — `AddAgentModal` only takes a name string, has no agent catalog integration |

**Conversation availability on disk** (important context):
- `dispatch_manager.save_invocation_state()` calls `persist_interim()` at the end of every
  turn in the tool-call loop. This writes `conversation.jsonl` after each LLM turn
  completes — so Idle and Completed invocations both have their full conversation on disk.
- Running invocations have conversation up to the last completed tool-cycle on disk; the
  in-progress delta arrives via `agent.output.chunk` streaming events.
- The JSONL path is:
  `<avix_root>/users/<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl`

---

## Files to Change / Create

### Rust — avix-core

**Step 1 — `crates/avix-core/src/invocation/store.rs`**

Add two methods to `impl InvocationStore`:

```rust
/// Return all invocation records whose session_id == session_id.
pub async fn list_for_session(&self, session_id: &str) -> Result<Vec<InvocationRecord>, AvixError>

/// Read the conversation.jsonl for an invocation and parse it as structured entries.
/// Returns an empty vec if the file does not exist (brand-new or pre-first-turn invocation).
pub async fn read_conversation(
    &self,
    id: &str,
    username: &str,
    agent_name: &str,
) -> Result<Vec<ConversationEntry>, AvixError>
```

`list_for_session`: full-scan of the redb table, collect where `record.session_id == session_id`.
`read_conversation`: build the file path
`users/<username>/agents/<agent_name>/invocations/<id>/conversation.jsonl`, read via
`LocalProvider`, parse each JSONL line as `ConversationEntry` (deserialize with
`serde_json::from_str`).

**Step 2 — `crates/avix-core/src/kernel/proc/mod.rs`**

Add two delegation methods to `impl ProcHandler`:

```rust
pub async fn list_invocations_for_session(
    &self,
    session_id: &str,
) -> Result<Vec<InvocationRecord>, AvixError>

pub async fn read_invocation_conversation(
    &self,
    invocation_id: &str,
) -> Result<Vec<ConversationEntry>, AvixError>
```

`list_invocations_for_session`: delegates to `invocation_store.list_for_session(session_id)`.
`read_invocation_conversation`: `get_invocation(id)` → extract `username`, `agent_name`
→ `invocation_store.read_conversation(id, username, agent_name)`.

**Step 3 — `crates/avix-core/src/kernel/ipc_server.rs`**

Add handler for `"kernel/proc/invocation-conversation"`:

```
params: { "id": "<invocation_uuid>" }
returns: JSON array of ConversationEntry objects
```

Also extend `"kernel/proc/invocation-list"` to accept an optional `session_id` param — when
present, delegate to `proc_handler.list_invocations_for_session(session_id)` instead of
`proc_handler.list_invocations(username, agent_name, live)`.

**Step 4 — `crates/avix-core/src/gateway/handlers/proc.rs`**

Extend the `match op` block:
- Add `"invocation-conversation"` → forward to `kernel/proc/invocation-conversation`
- Extend `"invocation-list"` branch: if `body["session_id"]` is non-empty, forward to
  `kernel/proc/invocation-list` (the existing handler will detect the session_id param and
  branch accordingly).

No new gateway files needed — one match arm each.

---

### Rust — avix-client-core

**Step 5 — `crates/avix-client-core/src/commands.rs`**

Add five public async functions:

```rust
pub async fn list_sessions(dispatcher, username: &str) -> Result<Vec<Value>, ClientError>
// dispatch(dispatcher, "proc", "session-list", json!({ "username": username }))

pub async fn get_session(dispatcher, session_id: &str) -> Result<Option<Value>, ClientError>
// dispatch(dispatcher, "proc", "session-get", json!({ "id": session_id }))

pub async fn resume_session(
    dispatcher,
    session_id: &str,
    input: &str,
) -> Result<Value, ClientError>
// dispatch(dispatcher, "proc", "session-resume", json!({ "session_id": session_id, "input": input }))

pub async fn get_invocation_conversation(
    dispatcher,
    invocation_id: &str,
) -> Result<Vec<Value>, ClientError>
// dispatch(dispatcher, "proc", "invocation-conversation", json!({ "id": invocation_id }))

pub async fn list_invocations_for_session(
    dispatcher,
    session_id: &str,
) -> Result<Vec<Value>, ClientError>
// dispatch(dispatcher, "proc", "invocation-list", json!({ "session_id": session_id }))
```

---

### Rust — avix-web routes

**Step 6 — `crates/avix-app/src-web/src/routes.rs`**

Add four new `match req.command.as_str()` arms:

| Command | What it does |
|---|---|
| `"list_sessions"` | `core_list_sessions(dispatcher, "")` → JSON-string response |
| `"get_session"` | `core_get_session(dispatcher, session_id)` → JSON-string or null |
| `"resume_session"` | `core_resume_session(dispatcher, session_id, input)` → pid |
| `"get_session_messages"` | see below |

`get_session_messages` takes `{ session_id }` as arg:
1. `core_list_invocations_for_session(dispatcher, session_id)` → vec of InvocationRecords
2. For each: `core_get_invocation_conversation(dispatcher, inv_id)` → vec of entries
3. Return a JSON array of `{ invocationId, agentName, status, entries: [...] }` objects

This collapses the multi-round-trip into one invoke call from the frontend. Because the
frontend makes exactly one call and gets all history, it works cleanly on reconnect.

---

### TypeScript — frontend

**Step 7 — `crates/avix-app/src/src/types/agents.ts`**

Add:
```typescript
export type SessionStatus = 'running' | 'idle' | 'paused' | 'completed' | 'failed' | 'archived';

export interface Session {
  id: string;
  title: string;
  goal: string;
  status: SessionStatus;
  summary?: string;
  originAgent: string;
  primaryAgent: string;
  participants: string[];
  ownerPid: number;
  pids: number[];
  lastUpdated: string;
  spawnedAt: string;
  tokensTotal: number;
}

export interface ConversationEntry {
  role: 'user' | 'assistant' | 'tool';
  content: string;
  toolCalls?: Array<{ id: string; name: string; args: string }>;
  filesChanged?: Array<{ path: string; diff?: string }>;
  thought?: string;
}

export interface InvocationMessages {
  invocationId: string;
  agentName: string;
  status: string;
  entries: ConversationEntry[];
}
```

Update `Page` to add `'session'`.

**Step 8 — `crates/avix-app/src/src/context/AppContext.tsx`**

Changes:
- Add `identity: string` to context value (default `''`)
- In `AppProvider`: call `invoke<AuthStatus>('auth_status')` on mount, store `identity`
- Add `sessions: Session[]` to state
- Add `selectedSessionId: string | null` and `setSelectedSession(id: string)` to context
- Load sessions on mount via `invoke<string>('list_sessions', {})` → `JSON.parse`
- Refresh sessions on `agent.spawned` and `agent.exit` events
- `setSelectedSession` sets `currentPage` to `'session'`
- Update `setPage` to clear `selectedSessionId` when switching away from `'session'`

Keep the existing `agents` state and `agent.output.chunk` streaming logic unchanged —
`SessionPage` will subscribe to the same streaming events and filter by the active session's
PIDs.

**Step 9 — `crates/avix-app/src/src/components/layout/Sidebar.tsx`**

Replace the "Agents" section (lines 115–178) with a "Sessions" section:

- **"New Session" button** at the top of the sessions section — clicking sets local state
  `newSessionOpen: true` which renders the `NewSessionModal`.
- For each session from `sessions` (filter: `status` in `['running', 'idle', 'paused']`):
  - Show title (first 40 chars of `session.title || session.goal`)
  - Status dot: green=running, amber=idle/paused
  - HIL count badge (from `notifications` filtered by session pids)
  - `onClick` → `setSelectedSession(session.id)`
  - Highlight if `selectedSessionId === session.id`
- Empty state: "No active sessions — click + to start one"
- Bottom nav stays as-is (Catalog / History / Services / Tools)

**Step 10a — New `crates/avix-app/src/src/components/NewSessionModal.tsx`**

Two-step wizard modal. Local state: `step: 'pick' | 'goal'`, `agents: InstalledAgent[]`,
`selected: InstalledAgent | null`, `goal: string`, `loading: boolean`, `search: string`.

_Step 1 — Agent picker_:
- On open: `invoke<string>('list_installed', {})` → parse → `setAgents`
- Search input filters by name/description
- Agent list: name, version badge, scope badge (`SYS`/`USR`), description preview
- Clicking a row: `setSelected(agent)`, pre-fills `goal` with `agent.description`, `setStep('goal')`
- Cancel button closes modal

_Step 2 — Goal input_:
- Header: selected agent name + scope badge
- Textarea pre-filled with `goal` (editable, placeholder: "What should this agent do?")
- "← Back" button returns to step 1
- "Start Session" button (disabled if `goal.trim()` empty or `loading`):
  ```
  const pidStr = await invoke<string>('spawn_agent', {
    name: selected.name,
    description: goal.trim(),
  });
  const pid = parseInt(pidStr, 10);
  // Refresh sessions, then navigate to the session whose ownerPid === pid
  await refreshSessions();   // calls invoke('list_sessions') and updates AppContext
  const session = sessions.find(s => s.ownerPid === pid);
  if (session) setSelectedSession(session.id);
  onClose();
  ```

Props: `isOpen: boolean`, `onClose: () => void`.

**Step 10 — New `crates/avix-app/src/src/pages/SessionPage.tsx`**

New file. Layout:

```
┌─ session header ──────────────────────────────────────────────────────────┐
│  Title / goal   [status badge]   [participant count]   [tokens]           │
├───────────────────────────────────────────────────────────────────────────┤
│  message thread (flex-col, scroll)                                        │
│                                                                           │
│  ┌── invocation block (per InvocationMessages) ──────────────────────┐   │
│  │  [agent-name label]                                                │   │
│  │  ● user message: "..."                                             │   │
│  │  ● assistant message: "..."                                        │   │
│  └────────────────────────────────────────────────────────────────────┘   │
│                                                                           │
│  ┌── live streaming block (if running invocation) ─────────────────┐    │
│  │  [agent-name label]                                              │    │
│  │  ● streaming text (from agent.output.chunk)                      │    │
│  └──────────────────────────────────────────────────────────────────┘    │
│                                                                           │
├───────────────────────────────────────────────────────────────────────────┤
│  [input bar / HIL widget / "Agent is running…" indicator]                 │
└───────────────────────────────────────────────────────────────────────────┘
         [optional collapsible multi-agent rail on right if participants > 1]
```

Logic:
- On mount (or when `selectedSessionId` changes): call `get_session_messages` → set
  `invocationMessages: InvocationMessages[]`; call `get_session` → set
  `session: Session`
- Also subscribe to `agent.output.chunk` and `agent.output.chunk` (is_final) for any
  PID in `session.pids` to show live output
- Subscribe to `hil.request` events — when received for a PID in `session.pids`, show
  `HilInlineCard` in the input area
- Input bar:
  - `session.status === 'idle'` → text input → on submit: `invoke('resume_session', { session_id, input })`
  - `session.status === 'running'` → text input (pipe) → on submit: `invoke('pipe_text', { pid: session.pids[0], text })`
  - pending HIL → `HilInlineCard` component (reuse existing)
  - `completed/failed` → no input, show a "Spawn new session" button

**Step 11 — `crates/avix-app/src/src/App.tsx`**

- Add `'session'` page route:
  ```tsx
  {currentPage === 'session' && <SessionPage />}
  ```
- Remove the `modalOpen` state, the `AddAgentModal` render, and the topbar "Add Agent"
  `onAddAgent` prop/handler — the `NewSessionModal` lives inside `Sidebar` and is the
  sole session-creation entry point.
- Update `AppContext` to expose a `refreshSessions()` function so `NewSessionModal` can
  trigger a reload after spawn without duplicating the fetch logic.

**Step 12 — `crates/avix-app/src/src/context/AppContext.tsx`** (addendum)

Add `refreshSessions: () => Promise<void>` to the context value. Expose the same
`invoke('list_sessions')` → parse → `setSessions` logic that runs on mount, so
`NewSessionModal` can call it after spawn.

**Step 13 — Remove `crates/avix-app/src/src/components/AddAgentModal.tsx`**

File is no longer referenced after removing the topbar button and updating `CatalogPage`.
Delete it. `CatalogPage`'s "Spawn" button should be updated to open `NewSessionModal`
with the agent pre-selected (skip to step 2 — pass `defaultAgent?: InstalledAgent` prop
to `NewSessionModal`).

---

## Implementation Order

1. `crates/avix-core/src/invocation/store.rs` — add `list_for_session` + `read_conversation`
2. `crates/avix-core/src/kernel/proc/mod.rs` — add `list_invocations_for_session` + `read_invocation_conversation`
3. `crates/avix-core/src/kernel/ipc_server.rs` — add `invocation-conversation` handler + extend `invocation-list` branch
4. `crates/avix-core/src/gateway/handlers/proc.rs` — add `invocation-conversation` op
5. `crates/avix-client-core/src/commands.rs` — add 5 new commands
6. `crates/avix-app/src-web/src/routes.rs` — add 4 new invoke handlers
7. `crates/avix-app/src/src/types/agents.ts` — add `Session`, `ConversationEntry`, `InvocationMessages` types; add `'session'` to `Page`
8. `crates/avix-app/src/src/context/AppContext.tsx` — add `identity`, `sessions`, `selectedSessionId`, `setSelectedSession`, `refreshSessions`
9. `crates/avix-app/src/src/components/NewSessionModal.tsx` — new file (agent picker → goal → spawn → navigate)
10. `crates/avix-app/src/src/components/layout/Sidebar.tsx` — session list + "New Session" button
11. `crates/avix-app/src/src/pages/SessionPage.tsx` — new file
12. `crates/avix-app/src/src/App.tsx` — add `'session'` route, remove `AddAgentModal` + topbar button
13. `crates/avix-app/src/src/pages/CatalogPage.tsx` — update "Spawn" button to open `NewSessionModal` with agent pre-selected
14. Delete `crates/avix-app/src/src/components/AddAgentModal.tsx`

---

## Testing Strategy

| Step | Test filter |
|---|---|
| 1 (store.rs) | `cargo test -p avix-core invocation::store` |
| 2 (proc/mod.rs) | `cargo test -p avix-core kernel::proc` |
| 3 (ipc_server.rs) | `cargo test -p avix-core kernel::ipc_server` |
| 4 (gateway proc handler) | `cargo test -p avix-core gateway::handlers::proc` |
| 5 (commands.rs) | `cargo test -p avix-client-core commands` |
| 6 (routes.rs) | `cargo test -p avix-web` |
| 7–14 (frontend) | Manual browser test + TypeScript build (`tsc --noEmit`) |

Target: existing tests must remain green; new unit tests for `list_for_session` and
`read_conversation` covering the happy path and empty-file case.

---

## Key Invariants to Preserve

- The gateway's `caller_identity` injection (empty `username` → gateway fills in) is
  already in place for `session-list`. The frontend sends no `username`; the gateway
  injects the caller's identity. Do not break this.
- `HistoryStore` (MessageRecord/PartRecord) is **not wired** and is out of scope for this
  plan. All message retrieval goes through `InvocationStore.read_conversation` (the JSONL
  path).
- `AgentThreadPage` and `HistoryPage` remain unchanged. `CatalogPage` keeps its agent list
  but its "Spawn" button is updated to open `NewSessionModal` (step 2 pre-selected) rather
  than the old `AddAgentModal`.
- The live `agent.output.chunk` streaming path in `AppContext` is unchanged; `SessionPage`
  piggybacks on it by filtering on `session.pids`.
- **New session auto-navigation**: `NewSessionModal` finds the new session by matching
  `session.ownerPid === pid` in the refreshed session list. The kernel guarantees
  `owner_pid` is the PID returned by the spawn call (set at session creation, never
  changes). This is a reliable lookup — no race condition.
- `AddAgentModal` is deleted in step 14 only after all references are removed in steps
  12–13. Do not delete early.
