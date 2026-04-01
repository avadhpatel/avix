# 10 — Tauri Client (Frontend)

## Frontend (React/Vite + TypeScript)

### Layout

Sidebar + main content area. Sidebar shows:
- **Agents** section — running agents (with HIL badge) + stopped agents
- **Bottom nav** — Catalog, History, Services, Tools pages

### Pages

| Page | Route key | Description |
|------|-----------|-------------|
| `AgentThreadPage` | `agent` | Selected agent's conversation thread + HIL cards + input bar |
| `CatalogPage` | `catalog` | Installed agents with `[SYS]`/`[USR]` badge; search; Spawn button per card |
| `HistoryPage` | `history` | Invocation table (ID, Agent, Status, Spawned, Tokens, Goal); click row → detail drawer with conversation |
| `ServicesPage` | `services` | System services table |
| `ToolsPage` | `tools` | Available tools, grouped by namespace |

### CatalogPage

- Calls `list_installed({ username: 'default' })` on mount
- Search filter across name and description fields
- Each card shows agent icon, name, version, scope badge, description
- **Spawn** button opens `AddAgentModal` pre-filled with `defaultName`

### HistoryPage

- Calls `list_invocations({ username, agentName? })` on mount; re-fetches on filter change
- Status badges: `running` (green), `completed` (blue), `failed` (red), `killed` (amber)
- **Detail drawer** (slide-in panel) — opened on row click:
  - Meta grid: agent, status, spawned, ended, tokens, tool calls
  - Goal and exit reason blocks
  - Full conversation messages rendered by role (`user` = blue, `assistant` = purple)

### State Management

- `AppContext` — agents, outputs, streaming, page routing
- `NotificationContext` — HIL/notification store, toast display
- Reducer-style updates via `useState` + `useCallback`

### Platform Abstraction

`platform/index.ts` detects runtime (Tauri vs browser) and exports unified `invoke`/`listen`:
- **Tauri**: `@tauri-apps/api/core` + `@tauri-apps/api/event`
- **Web**: HTTP POST `/api/invoke` + WebSocket `/api/events`

### Events (listen)

| Event | Payload | Effect |
|-------|---------|--------|
| `agent.spawned` | — | Refresh agent list |
| `agent.exit` | `{pid}` | Mark agent stopped |
| `agent.status` | `{pid, status}` | Update status badge |
| `agent.output` | `{pid, text}` | Append to output buffer |
| `agent.output.chunk` | `{pid, turn_id, text_delta, is_final}` | Stream live output |
| `notification` | notification object | Toast + store |
