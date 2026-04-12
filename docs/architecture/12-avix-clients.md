# 12 — Avix Clients

## avix-client-core Modules

- `atp/` — WS client (connect/send/next_frame, reconnect), `Dispatcher` (req/reply matching), `EventEmitter` (typed events: `agent.status`, `hil.request`, …), types (`Cmd`/`Frame`/`Reply`/`Event`)
- `commands.rs` — all ATP command helpers:
  - `spawn_agent(name, goal, caps)` → pid
  - `kill_agent(pid)` → `()`
  - `pipe_text(pid, text)` → `()`
  - `resolve_hil(pid, hil_id, approval_token, approved, note)` → `()`
  - `list_agents()` → `Vec<Value>`
  - `list_installed(username)` → `Vec<Value>` — installed agents via `proc/list-installed`
  - `list_invocations(username, agent_name?)` → `Vec<Value>` — history via `proc/invocation-list`
  - `list_invocations_live(username, agent_name?)` → `Vec<Value>` — includes running invocations
  - `list_invocations_for_session(session_id)` → `Vec<Value>` — filter by session via `proc/invocation-list { session_id }`
  - `get_invocation(id)` → `Option<Value>` — detail via `proc/invocation-get`
  - `get_invocation_conversation(invocation_id)` → `Vec<Value>` — parsed JSONL entries via `proc/invocation-conversation`
  - `list_sessions(username)` → `Vec<Value>` — active sessions via `proc/session-list`
  - `get_session(session_id)` → `Option<Value>` — detail via `proc/session-get`
  - `resume_session(session_id, input)` → `Value` — resume idle session via `proc/session-resume`
- `commands/spawn_agent.rs` — spawn with typed params
- `state.rs` — `AppState` (`RwLock`): config, dispatcher, emitter, `NotificationStore`, agents `Vec<ActiveAgent>`, connection_status, server_handle, pending_hils `hil_id→(pid,token)`, emit_callback
- `notification.rs` — `NotificationStore` (add/resolve/all)
- `persistence.rs` — save/load JSON (`notifications.json`, `layout.json`)
- `config.rs` — `ClientConfig`
- `server.rs` — `ServerHandle` (ensure_running daemon)

## CLI Subcommands (avix-cli)

### Agent commands

| Subcommand | Description |
|-----------|-------------|
| `avix agent spawn <name> --goal <goal>` | Spawn an agent instance |
| `avix agent list` | List active running agents |
| `avix agent kill <pid>` | Kill an agent by PID |
| `avix agent catalog [--username]` | List installed agents available to a user |
| `avix agent history [--agent] [--username]` | List invocation history |
| `avix agent show <id>` | Show invocation detail + conversation |

All agent commands accept `--json` for machine-readable output.

## Tauri Commands / Events

See `10-tauri-client.md` (frontend) and `11-tauri-backend.md` (backend).

## Daemon Runtime::start_daemon Phases

From `crates/avix-core/src/bootstrap/mod.rs`:

**bootstrap_with_root()** (pre-daemon):
1. Phase 0: init
2. Phase 1: VFS ephemeral (`/proc/`, `/kernel/`) — `phase1::run()`
3. Phase 2: `auth.conf` load, `AVIX_MASTER_KEY` → memory + zero env, persistent mounts — `phase2::mount_persistent_trees()`
4. Phase 3: mock service PIDs (logger, memfs, auth, router, tool-registry, llm, exec, mcp-bridge, gateway)

**start_daemon(port)**:
1. Phase 2: `kernel.agent` PID 1 spawn
2. Phase 3: services spawn (`llm.svc`, `router.svc`, …)
3. Phase 4: ATP gateway WS/TLS on port
4. Loop: poll `/run/avix/reload-pending` → `hot_reload()`
