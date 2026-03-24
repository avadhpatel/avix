# GUI App Gap 1: Tauri Workspace Setup

## Spec Reference
docs/spec/gui-cli-via-atp.md section 2/4 (workspace/avix-app).

## Tasks
1. Cargo.toml workspace: add \`crates/avix-app\`.
2. crates/avix-app/Cargo.toml: tauri deps (tauri 1.6+, tokio).
3. src-tauri/ standard: tauri.conf.json (dev/prod), Cargo.toml (tauri deps, client-core).
4. src-tauri/src/main.rs: tauri::Builder, AppState inject client-core AppState.
5. src/ React TS boilerplate (vite, golden-layout).
6. Test: \`cargo tauri dev\` launches window.

Verify: GUI shell opens, no ATP yet.

Est: 1h.