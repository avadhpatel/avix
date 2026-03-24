Avix Clients Architecture Specification File: docs/architecture/12-avix-clients.md Version: 1.0 (March 2026) Status: Final – ready for implementation
This single document combines everything we decided for both the rich desktop GUI (Tauri) and the CLI/TUI client. It introduces a shared crate so both clients reuse 80–90 % of the protocol, config, server control, reconnection, HIL, and notification logic.
1. Goals
	•	One codebase for config, server management, ATP WebSocket, command dispatching, event handling, HIL responses, and notifications.
	•	GUI (Tauri): beautiful floating+docking multi-agent dashboard with images, PDF, charts, iframes, toasts + notification center.
	•	CLI (Ratatui): powerful terminal dashboard (tabs/splits) + pure scripting mode (--json).
	•	Zero duplication of ATP protocol handling.
	•	Same UX patterns where possible (e.g. “+ Add Agent” flow, HIL handling, layout/session persistence).
	•	Works on macOS + Linux (Windows later).
2. Workspace Structure (updated)
avix/
├── Cargo.toml                          ← workspace members
├── crates/
│   ├── avix-core/
│   ├── avix-protocol/
│   ├── avix-client-core/               ← NEW SHARED CRATE (see section 3)
│   ├── avix-app/                       ← Tauri GUI (desktop binary)
│   │   ├── Cargo.toml
│   │   ├── src-tauri/                  ← standard Tauri folder
│   │   └── src/                        ← React + TS frontend
│   └── avix-cli/                       ← CLI binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── tui/
├── docs/architecture/
│   ├── 10-tauri-client.md              ← previous GUI frontend decisions
│   ├── 11-tauri-backend.md             ← previous GUI backend
│   └── 12-avix-clients.md              ← this file (master reference)
3. Shared Library: `avix-client-core`
Purpose: Everything that is common between GUI and CLI lives here.
Structure:
avix-client-core/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── config.rs               ← wraps avix-core config (init, load, save)
│   ├── server.rs               ← spawn/monitor "avix start" or RuntimeExecutor
│   ├── atp/
│   │   ├── client.rs           ← tokio-tungstenite WS + reconnect (60s grace)
│   │   ├── types.rs            ← all ATP structs (Cmd, Reply, Event, HilRequest, Notification, etc.)
│   │   ├── dispatcher.rs       ← route by domain/op → Reply
│   │   ├── event_emitter.rs    ← parse frames → typed events + notifications
│   │   └── notification.rs     ← unified Notification store + HIL logic
│   ├── state.rs                ← AppState (Arc>)
│   ├── persistence.rs          ← appDataDir helpers (notifications.json, layout.json)
│   └── commands.rs             ← high-level async functions (not Tauri-specific)
Key types exposed:
	•	AppState
	•	Cmd, Reply, Event, HilRequest, Notification { id, kind: Hil | AgentExit | SysAlert, … }
	•	Client (the WS handle)
Dependencies:
	•	avix-core, avix-protocol, tokio, tokio-tungstenite, serde, tauri (optional feature for GUI)
Both avix-app and avix-cli will depend on avix-client-core = { path = "../avix-client-core" }.
4. GUI Client – Tauri (`crates/avix-app`)
Frontend (src/ – React + TS)
	•	golden-layout (floating + docking style)
	•	Top-right + Add Agent button → modal (Name + Description + Advanced collapsed)
	•	Panel titles: agent-name – task summary + subtitle (status + info)
	•	Multiple panels per agent (bound by agentId + sessionId)
	•	ContentRenderer: images, PDF.js, Recharts, iframe
	•	Notifications: react-hot-toast (bottom-right) + Bell icon → notification center (popover or floating panel)
	•	HIL: per-panel orange banner + sticky toast (Approve/Deny) + notification center queue for multiples/escalations
	•	Layout persistence: appDataDir/ui-layout.json (C + D + E level – geometry + agent binding + task + focus state + global prefs)
	•	Initial layout rules (first-time / returning / reboot) exactly as decided
Backend (src-tauri/src/)
	•	Uses avix-client-core for 95 % of logic
	•	Only thin Tauri layer: main.rs, commands.rs (invoke handlers), event_emitter bridge to tauri::Emitter
	•	AppState injected via Tauri
5. CLI Client – Ratatui (`crates/avix-cli`)
Modes:
	•	Default: interactive TUI (Ratatui)
	•	--json / --quiet: pure scripting mode (machine-readable stdout)
TUI Structure (src/tui/):
	•	Vertical split: Sidebar (left) + Main area (tabs or splits for agents)
	•	Sidebar: same sections as GUI (Header → + Add Agent → Active Agents → Recent → System status)
	•	Agent “panels”: tabs or resizable splits (Ratatui layout)
	•	Multiple views per agent supported (via session-id)
	•	Output: scrollable text widget + markdown-ish rendering + ASCII tables for charts
	•	Rich content fallback: “Saved to ./output.pdf – open with external tool?” + xdg-open / open
	•	HIL: full-screen modal popup (blocks until resolved or timeout) + bottom notification bar
	•	Notifications: bottom status bar + popup list (same unified store from avix-client-core)
	•	Keyboard: Ctrl+N = new agent, j/k navigation, Enter = focus panel, etc.
	•	Persistence: optional simple layout save in appDataDir (tabs order + agent bindings)
Shared flow with GUI:
	•	Exactly the same avix-client-core for config, server start, WS, HIL response, notifications.
6. Unified Flows (implemented once in `avix-client-core`)
	1	Config-first → config::init() (works before server)
	2	Server start → server::start() (spawns runtime or direct executor)
	3	ATP connect → atp::client::connect() + auto-reconnect
	4	Agent spawn → same modal/form logic (GUI) or TUI form (CLI) → atp_send_cmd(proc.spawn)
	5	Streaming → event_emitter fans out agent.output to all subscribed panels/views
	6	HIL → notification::add_hil() + emit → GUI (toast + banner) or CLI (modal)
	7	Notifications → unified store → GUI (toast + center) or CLI (bar + popup)
	8	Reboot / reconnect → restore layout + pull latest HILs from server (no conversation history)
7. Persistence (unified)
	•	Location: Tauri’s app_data_dir() logic (reused in CLI via same helper)
	•	Files:
	◦	notifications.json
	◦	ui-layout.json (GUI full golden-layout + CLI simplified tab state)
	•	Loaded on startup in both clients
8. Implementation Order (recommended)
	1	Create avix-client-core + move shared logic there (start with config, server, atp::client, types).
	2	Update Tauri backend to depend on it (remove duplication).
	3	Finish Tauri GUI (you already have most decisions).
	4	Add avix-cli crate + basic clap + Ratatui skeleton.
	5	Wire CLI to same avix-client-core.
	6	Implement HIL + notifications in shared crate (both clients get it for free).
9. Cargo.toml snippets
Root workspace:
members = ["crates/avix-core", "crates/avix-protocol", "crates/avix-client-core", "crates/avix-app", "crates/avix-cli"]
avix-client-core/Cargo.toml:
[dependencies]
avix-core = { path = "../avix-core" }
avix-protocol = { path = "../avix-protocol" }
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["rustls"] }
serde = { version = "1", features = ["derive"] }
avix-app and avix-cli both add:
avix-client-core = { path = "../avix-client-core" }

This combined spec is now complete and self-contained. You can copy it directly into docs/architecture/12-avix-clients.md.
Next step: Shall I give you the exact starter code for the new avix-client-core crate (lib.rs + first modules) so you can begin extracting the shared logic immediately? Or would you prefer the CLI TUI skeleton first?
Just say the word and we’ll keep moving at the same detailed pace. 🚀

