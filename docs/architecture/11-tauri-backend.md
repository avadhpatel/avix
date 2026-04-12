# 11 — Tauri Backend

## Tauri Rust Backend

- Manages `SharedState<AppState>` (config, dispatcher, emitter, notifications, agents, pending_hils)
- **Commands** (invoke_handler):

| Command | Params | Returns | Notes |
|---------|--------|---------|-------|
| `spawn_agent` | `{name, description}` | pid str | Spawns a new agent instance |
| `resolve_hil` | `id str, approve bool` | `()` | Approves or denies a HIL request |
| `list_agents` | — | agents JSON str | Active running agents |
| `pipe_text` | `{pid, text}` | `()` | Send text input to agent (SIGPIPE) |
| `get_notifications` | — | notifs JSON | All notifications |
| `save_layout` | `layout_json str` | `()` | Persist UI layout |
| `auth_status` | — | `{authenticated, identity}` JSON | Check login state |
| `login` | `{identity, credential, save}` | `()` | Authenticate |
| `list_installed` | `{username}` | installed agents JSON str | Agents available to spawn (`proc/list-installed`) |
| `list_invocations` | `{username, agent_name?}` | invocation records JSON str | History (`proc/invocation-list`) |
| `get_invocation` | `{invocation_id}` | invocation JSON str or null | Detail (`proc/invocation-get`) |
| `list_sessions` | `{}` | sessions JSON str | Active sessions for authenticated user (`proc/session-list`); gateway injects caller identity |
| `get_session` | `{session_id}` | session JSON str or null | Single session detail (`proc/session-get`) |
| `resume_session` | `{session_id, input}` | result JSON str | Resume idle session with new input (`proc/session-resume`) |
| `get_session_messages` | `{session_id}` | `InvocationMessages[]` JSON str | All invocations + conversations for a session in one call |

### `get_session_messages` detail

This command collapses multiple round-trips into a single invoke call:

1. `list_invocations_for_session(dispatcher, session_id)` → vec of InvocationRecords
2. For each record: `get_invocation_conversation(dispatcher, inv_id)` → vec of ConversationEntry
3. Returns a JSON array of `{ invocationId, agentName, status, entries: [...] }` objects

This is the primary data-fetch for `SessionPage` on mount.

- **Events**: `emit(event, data)` from `AppState.emit_callback` → frontend `listen()`
- Auto server start if not running (`ServerHandle::ensure_running`)

See `crates/avix-app/src-web/src/routes.rs` for implementation (avix-web) and `crates/avix-app/src-tauri/src/commands.rs` for the Tauri desktop implementation.
