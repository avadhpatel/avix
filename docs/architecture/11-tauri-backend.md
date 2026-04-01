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
| `get_invocation` | `{invocation_id}` | invocation JSON str or null | Detail + conversation (`proc/invocation-get`) |

- **Events**: `emit(event, data)` from `AppState.emit_callback` → frontend `listen()`
- Auto server start if not running (`ServerHandle::ensure_running`)

See `crates/avix-app/src-tauri/src/commands.rs` for implementation.
