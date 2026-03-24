# GUI App Gap 1: Tauri Workspace Setup

## Spec Reference
docs/spec/gui-cli-via-atp.md sections:
* s2 Workspace: avix/Cargo.toml members += [\"crates/avix-app\"], crates/avix-app/ structure (Cargo.toml, src-tauri/, src/ React+TS).
* s4 GUI Client: Frontend (src/) golden-layout, Backend (src-tauri/src/) thin Tauri layer + AppState.
* s9 Cargo.toml: root workspace members, avix-app deps on avix-client-core (added later).

## Goals
* Initialize Tauri app crate in avix workspace.
* Set up standard src-tauri/ backend and src/ React+Vite frontend skeleton.
* Enable `cargo tauri dev` to launch empty GUI window.
* Prepare structure for avix-client-core integration.

## Dependencies
* avix-core, avix-protocol (workspace members).
* Tauri 1.6+, Vite, React, TypeScript, golden-layout (npm).

## Files to Create/Edit
* Cargo.toml (root workspace)
* crates/avix-app/Cargo.toml
* crates/avix-app/src-tauri/tauri.conf.json
* crates/avix-app/src-tauri/Cargo.toml
* crates/avix-app/src-tauri/src/main.rs
* crates/avix-app/src/ (index.html, vite.config.ts, App.tsx w/ golden-layout init)
* crates/avix-app/src-tauri/capabilities/ (default.json for commands)

## Detailed Tasks
1. Update root Cargo.toml:
```
toml
members = [
  \"crates/avix-core\",
  \"crates/avix-protocol\",
  \"crates/avix-app\",
]
```
   (Comment: avix-client-core/avix-cli later in gap2+)

2. Create crates/avix-app/Cargo.toml (standard Tauri app):
```
toml
[package]
name = \"avix-app\"
version = \"0.1.0\"
edition = \"2021\"

[build-dependencies]
tauri-build = { version = \"1.6\", features = [] }

[dependencies]
tauri = { version = \"1.6\", features = [ \"shell-open\", \"system-tray\" ] }
serde = { version = \"1.0\", features = [\"derive\"] }
serde_json = \"1.0\"
tokio = { version = \"1\", features = [\"full\"] }
```
   (avix-client-core dep added in gap3)

3. Setup src-tauri/:
   * tauri.conf.json: devPath \"http://localhost:1420\", prod bundle, macOS/Linux icons, window size 1400x900 resizable.
   * src-tauri/Cargo.toml: [dependencies] tauri = { version = \"1.6\", features = [] } (backend deps).
```
toml
[dependencies]
tauri = { version = \"1.6\", features = [\"api-all\"] }
# avix-client-core = { path = \"../avix-client-core\" }  # gap3
```
   * capabilities/default.json: allow-list for fs, http, shell.

4. src-tauri/src/main.rs skeleton:
```
rust
#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]

use tauri::{Manager, State};

#[derive(Clone)]
struct AppState {
    // avix-client-core::AppState placeholder
}

fn main() {
    tauri::Builder::default()
        .manage(AppState { /* default */ })
        .run(tauri::generate_context!())
        .expect(\"error running tauri\");
}
```
   (Expand with commands.rs in gap3)

5. src/ React+Vite boilerplate:
   * vite.config.ts: React TS plugin, port 1420.
   * src/App.tsx: GoldenLayout root div, top-right +Add button (disabled), empty panels.
```
tsx
import GoldenLayout from \"golden-layout\";

const layout = new GoldenLayout({
  root: { type: \"row\", content: [] },
});
layout.init();
```

6. `cargo tauri dev`: Frontend hot-reload, Rust backend compiles, window opens.

## Verify
* `cargo tauri dev` launches 1400x900 window with GoldenLayout shell.
* No crashes, console clean, no ATP logic yet.
* Drag/drop panels work (empty).

Est: 1h