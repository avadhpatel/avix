# GUI App Gap 3: Tauri Backend Bridge

## Spec Reference
section 4 Backend.

## Tasks
1. src-tauri/src/commands.rs: tauri::command handlers (spawn_agent, resolve_hil, list_agents).
2. Event bridge: client-core EventEmitter → tauri::Emitter (agent.output, hil.request).
3. AppState: inject Arc<AppState> to commands.
4. Persistence: layout.json via client-core.
5. Test: unit invoke handlers.

Verify: \`cargo tauri dev\`, console commands work.

Est: 1h.