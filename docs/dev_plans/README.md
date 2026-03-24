# Development Plans

This folder is used for active development initiatives.
It may contain markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Active Plans

### VFS Persistence

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `fs-gap-E-local-provider.md` | `StorageProvider` trait + `LocalProvider` (disk-backed) + `VfsRouter` replacing `Arc<MemFs>`; Phase 2 bootstrap mounts `/users/`, `/crews/`, `/services/` to disk | **Critical** | — |

Must be completed before any memory persistence is meaningful. Without it, all memory
records are lost on every avix restart.

---

### GUI + CLI Clients via ATP (spec: `docs/spec/gui-cli-via-atp.md`)

Implement `avix-client-core` shared library, wire it into `avix-cli` (scripting + TUI),
and lay the groundwork for the Tauri GUI backend. Implement in order A → H.

| File | What it builds | Priority | Depends On |
|------|---------------|----------|------------|
| `client-gap-A-avix-client-core-scaffold.md` | New `avix-client-core` crate + all ATP wire types (`Cmd`, `Reply`, `Event`, `Frame`, 16 event kinds, `LoginRequest/Response`) | **Critical** | — |
| `client-gap-B-atp-websocket-client.md` | `AtpClient` (HTTP login + WS upgrade + bearer auth) + `Dispatcher` (request/reply correlation, event broadcast) | **Critical** | A |
| `client-gap-C-atp-event-emitter-reconnect.md` | `EventEmitter` typed fan-out + reconnect with exponential backoff (capped 60 s) | **High** | B |
| `client-gap-D-notification-store-hil-persistence.md` | `NotificationStore` + `HilState` machine + `persistence.rs` (atomic JSON save/load for `notifications.json`, `layout.json`) | **High** | A |
| `client-gap-E-appstate-config-server.md` | `AppState` + `ClientConfig` + `ServerHandle` (auto-start `avix start`) + `commands.rs` (spawn agent, send signal, resolve HIL) | **High** | B, C, D |
| `client-gap-F-cli-atp-connect-scripting.md` | New ATP subcommands (`connect`, `agent list/spawn/kill/pipe`, `hil list/approve/deny`, `logs --follow`) + `--json` scripting mode | **High** | E |
| `client-gap-G-cli-tui-skeleton.md` | Ratatui TUI skeleton: sidebar + main area + tab bar + status bar, key bindings, layout unit tests | **Medium** | F |
| `client-gap-H-cli-tui-live-events-hil.md` | Live agent output streaming, HIL full-screen modal, notification popup, "new agent" form wired to ATP events | **Medium** | C, D, G |

---

---

## Development Workflow

After each gap plan is implemented and all tests pass:

1. Run the full verification suite:
   ```bash
   cargo test --workspace          # all tests must pass
   cargo clippy --workspace -- -D warnings  # zero warnings
   cargo fmt --check               # zero formatting diff
   ```
2. Commit the completed gap with a descriptive message, e.g.:
   ```bash
   git commit -m "Implement snapshot-gap-A: SnapshotFile schema and SnapshotStore"
   ```

One commit per completed gap. Do not batch multiple gaps into a single commit.

Delete the plan file and update this README once the work is committed.

---

## Design Notes

### Conversation History vs Memory

`RuntimeExecutor` maintains `conversation_history: Vec<Message>` in-process and passes
it on every `llm/complete` call (stateless LLMs require full context per call). This
in-session history is **ephemeral** — it is never persisted to VFS or stored in
memory.svc. It is discarded when the executor exits.

`memory.svc` is a **separate, complementary layer**:
- **In-session:** `conversation_history` (held by RuntimeExecutor, passed to every LLM call)
- **Cross-session:** memory.svc episodic/semantic/preference records (VFS-persisted,
  injected as a summary block at the next spawn)

At session end (SIGSTOP with `autoLogOnSessionEnd: true`), the executor asks the LLM
to summarise `conversation_history` and writes that summary — not the raw transcript —
via `memory/log-event`. The raw history is then discarded.

This design means agents gain continuity across sessions without unbounded context
growth. The LLM summary is the cross-session artifact; the full transcript is ephemeral.
