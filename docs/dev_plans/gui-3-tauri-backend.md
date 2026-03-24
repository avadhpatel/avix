# GUI App Gap 3: Tauri Backend Bridge to client-core

## Spec Reference
docs/spec/gui-cli-via-atp.md sections:
* s3 client-core: AppState(Arc), Client WS handle, Commands (spawn_agent etc.).
* s4 GUI Backend: thin Tauri layer - main.rs injects AppState, commands.rs invoke handlers, EventEmitter -> tauri::Emitter.
* s6 Flows: server start, ATP connect, HIL resolve via commands.
* s9 Cargo: avix-app/avix-cli dep avix-client-core = { path = \"../avix-client-core\" }.

## Goals
* Integrate avix-client-core into Tauri backend.
* Expose high-level commands to frontend (spawn_agent, resolve_hil, get_notifications).
* Bridge client-core events/notifications to frontend via tauri::emit.
* Handle persistence via shared state.

## Dependencies
* avix-client-core (path ../avix-client-core).
* tauri api-all features.

## Files to Create/Edit
* crates/avix-app/src-tauri/Cargo.toml: add avix-client-core dep.
* crates/avix-app/src-tauri/src/main.rs: manage(client_core::AppState::new())
* crates/avix-app/src-tauri/src/commands.rs
* crates/avix-app/src-tauri/src/lib.rs (if mod commands;)

## Detailed Tasks
1. src-tauri/Cargo.toml: add dep:
```
toml
avix-client-core = { path = \"../avix-client-core\" }
```

2. Update src-tauri/src/main.rs:
```
rust
use avix_client_core::AppState;

#[tauri::command]
async fn spawn_agent(state: State<AppState>, name: String, desc: String) -> Result<String, String> {
    state.spawn_agent(name, desc).await.map(|id| id.to_string())
}

#[tauri::command]
async fn resolve_hil(state: State<AppState>, id: String, approve: bool) -> Result<(), String> {
    let id = Uuid::parse_str(&id)?;
    state.resolve_hil(id, approve).await
}

// more: list_agents, get_notifications, save_layout etc.

fn main() {
    tauri::Builder::default()
        .manage(avix_client_core::AppState::new().expect(\"init state\"))
        .invoke_handler(tauri::generate_handler![spawn_agent, resolve_hil /* more */])
        .run(tauri::generate_context!())
        .expect(\"error\");
}
```
   * Auto-start server/connect in AppState::new().

3. Event bridge: in client-core EventEmitter subscribe:
```
rust
// In AppState init or separate task
let state = state.clone();
event_emitter.on_event(move |event| {
    app.emit(\"agent-event\", &event).unwrap();
});
event_emitter.on_notification(move |notif| {
    app.emit(\"notification\", &notif).unwrap();
});
```
   * Use tauri::AppHandle passed to AppState or separate setup.

4. Persistence: commands save_layout(layout_json: String), uses state.persistence.save().

5. Unit tests: #[cfg(test)] mock AppState, invoke_handler test.

## Verify
* `cargo tauri dev`: backend compiles w/ client-core, no runtime errors.
* Browser console: invoke('spawn_agent', {name:'test'}) → events emitted.
* Persistence files created in appDataDir.

Est: 1.5h