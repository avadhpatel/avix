# Development Plans

This folder is used for active development initiatives.
It may contain markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Active Plans

### Packaging & Installation System (spec: `docs/architecture/15-packaging.md`)

Standardized `.tar.xz` packaging, ATP-first install flow, and user-facing CLI/TUI/Web-UI surfaces.
Implemented in order A → F. All gaps completed.

| File | Status | What it builds |
|------|--------|----------------|
| `pkg-gap-A-kernel-package-handlers.md` | ✅ Done | `PackageSource` resolver, xz decompression, `AgentInstaller`, `proc/package/install-*` syscalls |
| `pkg-gap-B-cli-commands.md` | ✅ Done | `avix agent install`, `avix service install` CLI, live progress |
| `pkg-gap-C-tui-webui-github-actions.md` | ✅ Done | TUI `:install`, Web-UI Extensions tab, GitHub Actions workflow |
| `pkg-gap-D-gpg-rollback-polish.md` | ✅ Done | Atomic install + rollback, install quota, `uninstall` commands |
| `pkg-gap-E-authoring-tooling.md` | ✅ Done | `PackageValidator`, `PackageBuilder`, `PackageScaffold`, `avix package` CLI |
| `pkg-gap-F-third-party-trust-keyring.md` | ✅ Done | TrustStore, TrustedKey, GPG verification, `avix package trust` CLI |

**Status:** Complete — pkg-gaps A–F done, incorporated into `docs/architecture/15-packaging.md`

---

### Session Management (spec: `docs/architecture/14-agent-persistence.md`)

Implement first-class Session abstraction for multi-turn agent workflows.
Phase 1 delivers Sessions v1.0: Idle status, SessionRecord persistence, and session-aware spawn/resume.

| File | What it builds | Priority | Depends On |
|------|---------------|----------|------------|
| `session-gap-A-sessions-v1.md` | `Idle` status for Invocations/Sessions, `SessionRecord` + `SessionStore` (redb + VFS), auto-session on spawn, ATP `session-*` ops, CLI `session *` commands | **High** | — |

**Status:** Phase 1 (Tasks 1-3) complete — Types and persistence layer implemented

---

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

### Service Authoring (spec: `docs/spec/service-authoring.md`)

Implement the full service lifecycle: `service.unit` parsing, process spawning, tool
descriptor scanning, installation pipeline, CLI management, `_caller` injection, and
restart/secrets. Implement in order A → H.

| File | What it builds | Priority | Depends On |
|------|---------------|----------|------------|
| `svc-gap-A-service-unit-parser.md` | `ServiceUnit` TOML parser + all section types (`RestartPolicy`, `HostAccess`, `RunAs`, `JobsSection`) + `InstallReceipt` + `parse_duration` | **Critical** | — |
| `svc-gap-B-service-process-spawner.md` | `ServiceProcess` OS spawn + env injection + `ServiceStatus` VFS file + `discover_installed` + Phase 4 bootstrap | **Critical** | A |
| `svc-gap-C-tool-descriptor-scanner.md` | Typed `ToolDescriptor` + `ToolScanner` (reads `*.tool.yaml`) + wire into `handle_ipc_register` | **High** | A, B |
| `svc-gap-D-service-installer.md` | `ServiceInstaller` (fetch, SHA-256 verify, tar extract, conflict check, receipt) + `sys/install` syscall handler | **High** | A |
| `svc-gap-E-cli-service-commands.md` | `avix service install/list/status/start/stop/restart/uninstall/logs` subcommands | **High** | D, client-gap-F |
| `svc-gap-F-ipc-tool-add-remove-wire.md` | Wire `ipc.tool-add/remove` JSON-RPC methods + typed params + `drain` semantics + `tool.changed` ATP event | **High** | A, B, C |
| `svc-gap-G-caller-injection.md` | `CallerInfo` struct + `caller_scoped` in `ServiceRecord` + router dispatcher injection + `ServiceSpawnRequest::from_unit` | **Medium** | A, F |
| `svc-gap-H-restart-watchdog-secrets.md` | `ServiceWatchdog` background task + `kernel/secret/get` IPC method + `avix secret set --for-service` CLI | **Medium** | A, B |

---

### Agent History Persistence v2 (spec: `docs/specs/agent-history-persistence-v2.md`)

Implement v2 of the agent run history system: live/interim persistence and richer conversation data (v2.0), followed by hierarchical sessions (v2.1).

| File | What it builds | Priority | Depends On |
|------|---------------|----------|------------|
| `history-v2-gap-A-interim-snapshots.md` | `persist_interim()` in `InvocationStore`, RuntimeExecutor snapshot hook after N tool calls, SIGSAVE triggers | **High** | — |
| `history-v2-gap-B-structured-conversation.md` | `ConversationEntry` with tool_calls, files_changed, thought fields; backward-compatible JSONL format | **High** | A |
| `history-v2-gap-C-atp-live-history.md` | ATP handlers: `proc/invocation-snapshot`, `proc/invocation-get --live`, `proc/invocation-list --live` | **High** | A, B |
| `history-v2-gap-D-hierarchical-sessions.md` | `MessageRecord`, `PartRecord`, `HistoryStore`; migration from existing invocations; ATP `message-*`, `part-*` ops | **Medium** | A, B, C |

**Status:** Draft — implementation not yet started

---

### workspace.svc (spec: `docs/spec/workspace-svc.md`)

High-level workspace abstraction service with automatic session history integration for project-centric file operations.

| File | What it builds | Priority | Depends On |
|------|---------------|----------|------------|
| `workspace-gap-A-service-skeleton.md` | Service skeleton, registration, IPC listener, basic read tools (list/read/info), VFS integration, history skeleton | **High** | History v2 |
| `workspace-gap-B-write-operations.md` | Write operations + caller extraction + create-project + delete tools | **High** | A |
| `workspace-gap-C-snapshot-search.md` | Snapshot, search, set-default tools | **Medium** | B |

**Status:** All gaps completed ✓

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

---

## To-Do List

- Convert 'auth.conf' file to 'auth.yaml' - everything else is yaml format
- Separate client.json and server.json - and move both to yaml
- **`cap/request-tool` fixes** (`cap-request-tool-fixes.md`): (1) HIL flow is a stub —
  `KernelResourceHandler` always returns `granted: false` without triggering SIGPAUSE /
  `HilManager::open` / ATP event / SIGRESUME; (2) silent denial when `resource_handler`
  is None; (3) agent re-requests denied tools within the same turn — record denials in
  `denied_tools`, surface in prompt Block 4, clear on new user message.

