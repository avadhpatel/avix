# 10 — Tauri Client (Frontend)

## Frontend (React/Vite + TypeScript)

### Layout

Sidebar + main content area.

**Sidebar** shows:
- **Sessions** section — active sessions (Running/Idle/Paused) with status dots and HIL count badges; "New Session" (+) button at the top
- **Bottom nav** — Catalog, History, Services, Tools pages

The old "Agents" section and topbar "Add Agent" button have been removed. Sessions are the primary unit shown in the sidebar. `NewSessionModal` is the sole entry point for creating a session.

### Pages

| Page | Route key | Description |
|------|-----------|-------------|
| `SessionPage` | `session` | Per-session conversation thread + live streaming + HIL + input bar |
| `AgentThreadPage` | `agent` | Legacy per-agent thread (used for raw PID-based navigation) |
| `CatalogPage` | `catalog` | Installed agents with `[SYS]`/`[USR]` badge; search; Spawn opens `NewSessionModal` |
| `HistoryPage` | `history` | Invocation table; click row → detail drawer with conversation |
| `ServicesPage` | `services` | System services table |
| `ToolsPage` | `tools` | Available tools, grouped by namespace |

### SessionPage

- On mount (or `selectedSessionId` change): calls `get_session` + `get_session_messages`
- `get_session_messages` returns `InvocationMessages[]` — one block per invocation, each with `agentName`, `status`, and `entries: ConversationEntry[]`
- Historical blocks rendered above live streaming block
- Live streaming from `agent.output.chunk` events filtered to `session.pids`
- Context-aware input bar:
  - `idle` → resume via `resume_session(session_id, input)`
  - `running` → pipe via `pipe_text(pid, text)`
  - pending HIL on any session PID → `HilInlineCard`
  - `completed`/`failed`/`archived` → read-only; "Spawn new session" button
- Optional collapsible multi-agent rail (right side) when `session.participants.length > 1`

### NewSessionModal

Two-step wizard (component: `components/NewSessionModal.tsx`):

1. **Agent picker** — `list_installed` on open; search filter; scope badges; click to advance
2. **Goal input** — pre-filled from agent description; "Start Session" calls `spawn_agent`; after spawn: refresh sessions, match `session.ownerPid === pid`, navigate to session

`CatalogPage`'s Spawn button opens `NewSessionModal` with `defaultAgent` pre-set (skips step 1).

### CatalogPage

- Calls `list_installed({})` on mount
- Search filter across name and description fields
- Each card shows agent icon, name, version, scope badge, description
- **Spawn** button opens `NewSessionModal` with `defaultAgent` set — opens directly at goal-input step

### HistoryPage

- Calls `list_invocations({ username, agentName? })` on mount; re-fetches on filter change
- Status badges: `running` (green), `completed` (blue), `failed` (red), `killed` (amber)
- **Detail drawer** (slide-in panel) — opened on row click:
  - Meta grid: agent, status, spawned, ended, tokens, tool calls
  - Goal and exit reason blocks
  - Full conversation messages rendered by role

### State Management

- `AppContext` — agents, outputs, streaming, page routing, **identity**, **sessions**, **selectedSessionId**, **setSelectedSession**, **refreshSessions**
- `NotificationContext` — HIL/notification store, toast display
- Reducer-style updates via `useState` + `useCallback`

#### AppContext additions

| Field / Method | Type | Description |
|---|---|---|
| `identity` | `string` | Authenticated username — loaded from `auth_status` on mount |
| `sessions` | `Session[]` | Active sessions — loaded on mount; refreshed on `agent.spawned` / `agent.exit` |
| `selectedSessionId` | `string \| null` | Currently open session |
| `setSelectedSession(id)` | `(id: string) => void` | Navigate to session page |
| `refreshSessions()` | `() => Promise<void>` | Re-fetch `list_sessions` and update state |

### TypeScript Types

`src/types/agents.ts`:

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

### Platform Abstraction

`platform/index.ts` detects runtime (Tauri vs browser) and exports unified `invoke`/`listen`:
- **Tauri**: `@tauri-apps/api/core` + `@tauri-apps/api/event`
- **Web**: HTTP POST `/api/invoke` + WebSocket `/api/events`

### Events (listen)

| Event | Payload | Effect |
|-------|---------|--------|
| `agent.spawned` | — | Refresh agent list + sessions |
| `agent.exit` | `{pid}` | Mark agent stopped + refresh sessions |
| `agent.status` | `{pid, status}` | Update status badge |
| `agent.output` | `{pid, text}` | Append to output buffer |
| `agent.output.chunk` | `{pid, turn_id, text_delta, is_final}` | Stream live output; `SessionPage` filters by `session.pids` |
| `notification` | notification object | Toast + store |
