# 11-tauri-backend COMPLETE

Updated for gaps 1-6.

## Tauri Rust Backend

- Manages `SharedState<AppState>` (config, dispatcher, emitter, notifications, agents, pending_hils)
- **Commands** (invoke_handler):
  | Cmd | Params | Returns |
  |-----|--------|---------|
  | spawn_agent | {name, description} | pid str |
  | resolve_hil | id str, approve bool | () |
  | list_agents | - | agents json str |
  | get_notifications | - | notifs json |
  | save_layout | layout_json str | () |
- **Events**: emit(event, data) from AppState.emit_callback → EventEmitter bridge
- Auto server start if not running (ServerHandle::ensure_running)