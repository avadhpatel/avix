**Token efficiency is critical.** Claude must follow the Strict Token-Saving Rules below on every single response.

# CLAUDE.md — Avix Development Instructions

> This file tells Claude Code how to work on this codebase. Read it fully at the start
> of every session before touching any code.

---

## What is Avix?

Avix is an **agent operating system** modelled on Unix/Linux primitives. Agents run as
processes with PIDs. The LLM is stateless — analogous to a CPU. The `RuntimeExecutor` is
the actual process — stateful, owns context, enforces policy. Services are traditional
deterministic software. The capability token system is the trust boundary.

| Linux concept    | Avix equivalent                                           |
|------------------|-----------------------------------------------------------|
| Kernel / PID 1   | `avix` runtime binary + `kernel.agent`                    |
| Processes        | Agents (LLM conversation loops + `RuntimeExecutor`)       |
| Filesystem       | MemFS — driver-swappable VFS                              |
| Syscalls         | `/tools/kernel/**` — 32 calls across 6 domains            |
| Shared libraries | Services exposing tools at `/tools/<namespace>/`          |
| IPC / sockets    | `router.svc` + platform-native local sockets              |
| Capabilities     | HMAC-signed `CapabilityToken` issued by `auth.svc`        |
| Signals          | `SIGSTART`, `SIGPAUSE`, `SIGRESUME`, `SIGKILL`, `SIGSTOP`, `SIGSAVE`, `SIGPIPE`, `SIGESCALATE` |
| /proc            | `/proc/<pid>/status.yaml`, `/proc/<pid>/resolved.yaml`    |
| /etc/passwd      | `/etc/avix/users.yaml`                                    |
| /etc/group       | `/etc/avix/crews.yaml`                                    |

**Authoritative references** (read these before implementing any subsystem):

- `docs/architecture/` — all architecture docs (00–09)
- `docs/architecture/07-services.md` — service lifecycle, `service.yaml` YAML, installation, `_caller` injection, watchdog, secrets
- `docs/architecture/08-llm-service.md` — llm.svc multi-modality spec
- `docs/architecture/09-runtime-executor-tools.md` — RuntimeExecutor tool exposure model

---

## Architecture Invariants

These are hard rules. Violating any of them is a bug, not a design choice.

### Boot & Config

1. `auth.conf` **must exist** before `avix start`. There is no setup mode inside core.
   Config is produced by `avix config init`. Bootstrap aborts immediately if missing.
2. `credential.type: none` **does not exist**. All auth is `api_key` or `password`.
3. `AVIX_MASTER_KEY` is read from the environment in Phase 2, held in memory only, and
   the env var is **zeroed immediately** after loading. It never touches disk.

### Communication Layers

4. **ATP = external** (WebSocket/TLS). **IPC = internal** (local sockets + JSON-RPC 2.0).
   These two protocols never cross the boundary. `gateway.svc` is the sole translator.
5. IPC transport is `local-ipc` — Unix domain sockets on Linux/macOS, Named Pipes on
   Windows. The kernel resolves the platform path. Config and service code use logical
   names only (`AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK`).
6. Every IPC message uses **4-byte little-endian length-prefix framing** over a
   **fresh connection per call**. No persistent multiplexed channels.
7. Long-running tools return `job_id` immediately; workers emit progress via `jobs.svc`.

### LLM and RuntimeExecutor

8. `llm.svc` **owns all AI inference**. `RuntimeExecutor` calls `llm/complete` (and other
   `llm/*` tools) via IPC — it never calls provider APIs directly.
9. **Kernel tool calls are deterministic** — they are made by kernel code, never
   LLM-decided. The LLM requests tools; `RuntimeExecutor` enforces policy.
10. The LLM **never sees raw capability tokens, IPC messages, or signal delivery**.
    Everything is mediated through the tool interface.

### Tool Naming

11. Tool names use **`/` as namespace separator** (`fs/read`, `mcp/github/list-prs`).
    Provider adapters mangle to `__` on the wire (`fs__read`) and unmangle on return.
    **No Avix tool name ever contains `__`** — this is reserved for wire mangling only.
12. **Category 2 tools** (`agent/`, `pipe/`, `cap/`, `job/`) are registered by
    `RuntimeExecutor` via `ipc.tool-add` at agent spawn and removed via `ipc.tool-remove`
    at exit. They are never hard-coded in any service's tool list.
13. **Always-present tools** (regardless of capability grants):
    `cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch`.

### Secrets

14. Secrets in `/secrets/` are **never readable via the VFS**. VFS reads of any path
    under `/secrets/` return `EPERM`. Secrets are kernel-injected into agent env at
    spawn only.

### Filesystem Ownership

15. The kernel **never writes** into user-owned trees (`/users/`, `/services/`, `/crews/`)
    **via the VFS ACL layer**. Exception: kernel components may write directly to disk via
    `LocalProvider` (which bypasses ACL), specifically for `InvocationStore` artefacts under
    `users/<username>/agents/`. This is intentional — the kernel is trusted.
16. Users and agents **never write** into ephemeral (`/proc/`, `/kernel/`) or system trees
    (`/bin/`, `/etc/avix/`).
17. Sessions live in `/proc/` — they are runtime state, never persisted under `/users/`.
18. Invocation records live in `users/<username>/agents/` — they are persistent, not ephemeral.
    A session and its invocation record are linked by `session_id` but have independent lifetimes.

---

## Strict Token-Saving Rules for Claude

Claude **must minimize token usage and avoid unnecessary work** at all times. The following rules are mandatory and override any default behavior:

- **Never explore or read files** unless they are explicitly listed in the current dev plan or the relevant architecture spec files.
- **Never run full workspace commands**: Prohibited commands include `cargo test --workspace`, `cargo test`, `cargo check --workspace`, `cargo clippy --workspace`, `cargo build --workspace`, or any command without a specific package or test filter.
- **Never suggest or make changes** to files not listed in the approved dev plan.
- **Never perform large refactors**, add extra features, improve unrelated code, or "future-proof" anything unless explicitly asked in the task.
- **Never write more code than necessary** to satisfy the current file's requirements in the dev plan.
- **Never output long explanations**, full file diffs, or speculative plans unless the user specifically requests them. Keep responses concise and action-focused.
- **In Mode 1**: Only create the dev plan file. Do not run any cargo commands or edit any source code.
- **In Mode 2**: Work on **exactly one file at a time**. After finishing the changes, tests, and compile check for that file, immediately stop and wait for user instructions before moving to the next file (even if the plan lists more files).
- **Testing is strictly targeted**: Only run tests that directly cover the code touched in the current file. Use precise filters (e.g. `cargo test crate_name::module::test_name`).
- **After completing an entire dev plan** in Mode 2: Update only the relevant architecture spec files, then immediately hand control back to the user. Do not run any additional commands or make any other changes.

Violating any of these rules wastes tokens and is considered incorrect behavior.

---

## Crate Structure

```
crates/
├── avix-core/     ← ALL logic lives here as a library. No binary targets.
├── avix-cli/      ← Thin CLI binary. No business logic. Calls avix-core.
├── avix-app/      ← Thin desktop app binary. No business logic.
└── avix-docker/   ← Thin headless binary for Docker. No business logic.
```

**The rule:** If it can be tested, it lives in `avix-core`. Binary crates are entry
points only — they parse CLI args, call `avix-core`, and exit.

---

## Development Workflow

NEVER USE grep or read commands, ONLY USE Serena LSP tools for all code reading.

Claude **operates in exactly one of two modes at any time**. It must never mix modes or perform actions from both modes in a single response. The user will explicitly indicate which mode to use (or the context will make it clear). Follow the chosen mode strictly and do not deviate.

### Mode 1: Feature Planning & Dev Plan Creation

Use this mode when the user requests a new feature, change, or task (or when no approved dev plan exists for the request).

1. **Understand the task at hand** — fully read and internalize the user's request. Ask clarifying questions if anything is ambiguous.
2. **Start with the architecture spec files** — immediately review all relevant documents in `docs/architecture/` (especially the numbered 00–09 series and any feature-specific specs) to identify every existing feature, component, invariant, and area that must be touched or extended.
3. **Confirm the features with the user** — explicitly list every feature, component, or behavior that needs to be updated or added according to the architecture specs. Present this list clearly to the user and obtain **explicit confirmation** before proceeding.
4. **Determine which files need to be changed** — based solely on the confirmed specs, identify the exact source files, test files, configuration files, or other files that must be created, modified, or deleted.
5. **Write the final update plan** — create a new Markdown file (or update an existing one) in the `docs/dev_plans/` folder **exactly** following the instructions in the "Development Plans" section of this document. The dev plan must include:
   - Clear task summary and the user-confirmed features
   - All architecture spec files referenced
   - Precise list of files to change/create/delete, with rationale
   - Step-by-step implementation order (one file at a time)
   - Targeted testing strategy (relevant tests only + target coverage requirements)

Do **not** make any code changes, run any commands, or edit any files outside of creating the dev plan until the user explicitly approves the plan and instructs you to switch to Mode 2.

### Mode 2: Feature Implementation from Dev Plan

Use this mode **only** when the user explicitly tells you to implement from an approved dev-plan file located in `docs/dev_plans/`.

1. Read the entire approved dev plan first.
2. Implement **strictly one file at a time**, following the exact order specified in the dev plan:
   - Make **only** the minimal code changes required for that specific file.
   - Ensure every change **compiles cleanly** (run `cargo check` or `cargo build` **only** on the affected crate/package).
   - Always add tracing logs to the changes at the right level for better
     debuggability, error tracking etc.
   - Add or update **only the necessary tests** needed to achieve the target test coverage for the code touched (see "Testing" section in Code Conventions).
   - Run **only the tests that apply to the code touched** (use precise filters such as `cargo test <module_path>::` or `cargo test --test <test_file>`). **Never** run full workspace tests, `cargo test`, or tests for untouched code.
   - Verify that the relevant tests pass.
3. After **all** files listed in the dev plan have been successfully implemented and their targeted tests pass:
   - Update the relevant architecture specification files in `docs/architecture/` to accurately document the changes made. Architecture specs must remain the single source of truth.
4. Stop. Do **not** perform any additional actions. Return control to the user and ask for the next instructions.

**Important rules that apply in both modes:**
- Claude must always stay in only one mode per response.
- Never run full workspace tests (`cargo test --workspace`, `cargo test`, etc.) as they are slow and token-heavy.
- Never edit architecture specs until Mode 2 is fully complete.
- Never create or edit code without an approved dev plan (except for creating the dev plan itself in Mode 1).


### Before Every Commit (targeted only)

After finishing changes for a specific file or completing a dev plan:
```bash
# Only on the exact crate and test filters for files changed in this step
cargo test <precise_test_filter>          # e.g. avix_core::some_module
cargo clippy --package <touched_crate> -- -D warnings
cargo fmt --check
```

---

## Code Conventions

Always make the **smallest possible change** that fulfills the dev plan for the current file. Do not add extra functionality, comments, or optimizations unless they are explicitly required. Prioritize compile success and targeted test coverage over elegance or completeness.

### Error Handling

- Use `thiserror` for library error types in `avix-core`
- Use `anyhow` for application-level errors in binary crates
- **Never use `.unwrap()`** in non-test code — use `?` or explicit error handling
- Every public function that can fail returns `Result<T, E>`

### TUI Invariants
* TUI loop non-blocking: 100ms poll + async actions via mpsc
* State via reducer pattern (immutable updates)
* ATP events dispatched to single background task
* Persist only notifications (not agent output/state)
* Modals exclusive: HIL &gt; form &gt; popup &gt; main UI

### TUI Key Bindings Reference
**Normal mode**:
* `q`: quit
* `c`: connect
* `/`: command mode
* ↑↓: agent select
* `a`: spawn test agent
* `f`: toggle new agent form
* `n`: toggle notifications popup

**Command mode (`:`)**:
* chars/backspace/←→/↑↓(history)/Enter/Esc

**HIL modal**: `a`(approve)/`d`(deny)/Esc(dismiss)
**Agent form**: Tab(switch)/Enter(spawn)/Esc/cancel
**Notifs popup**: ↑↓/Enter(read)/Esc

See `docs/architecture/tui.md`.

### Naming

| Context               | Convention           | Example                    |
|-----------------------|----------------------|----------------------------|
| Structs / enums       | `PascalCase`         | `RuntimeExecutor`          |
| Functions / variables | `snake_case`         | `spawn_with_registry`      |
| Constants             | `SCREAMING_SNAKE_CASE` | `MAX_FRAME_BYTES`        |
| IPC method names      | `namespace/verb`     | `kernel/proc/spawn`        |
| Config `kind` values  | `PascalCase`         | `KernelConfig`, `LlmConfig`|
| Tool names            | `namespace/verb`     | `fs/read`, `llm/complete`  |
| Wire-mangled names    | `namespace__verb`    | `fs__read` (adapter only)  |

### Async

- Use `tokio::test` for all async tests
- Use `Arc<RwLock<T>>` (tokio) for shared mutable state
- Prefer `tokio::spawn` for background tasks; hold the `JoinHandle`
- Never block the async runtime — use `tokio::task::spawn_blocking` for CPU-bound work

### Logging

- Use the `tracing` crate everywhere
- **Never use `println!`** in library code — use `tracing::info!`, `tracing::debug!`, etc.
- Structured JSON output in production; pretty output in dev
- Log at `debug!` for per-turn loop events, `info!` for lifecycle events

### Testing

- Target: **95%+ coverage** via `cargo tarpaulin`
- Unit tests go in the same file under `#[cfg(test)]`
- Integration tests go in `crates/avix-core/tests/`
- Always use `tempfile::tempdir()` for tests that need a filesystem root
- Always use `tokio::time::timeout` when testing async operations that might hang
- ATP E2E: `crates/avix-tests-integration` covers full WS cycle (auth/cmd/reply/event) + Gap4 ops/events.

---

## Performance Targets

These are hard benchmarks, not aspirational.

| Operation                          | Target   |
|------------------------------------|----------|
| Boot to ready                      | < 700 ms |
| ATPToken validation                | < 50 µs  |
| IPC frame encode + decode          | < 10 µs  |
| IPC round-trip (local socket)      | < 500 µs |
| VFS file read (in-memory)          | < 50 µs  |
| Tool registry lookup               | < 5 µs   |
| Provider adapter tool translation  | < 5 µs   |
| Tool name mangle (`/` → `__`)      | < 1 µs   |
| Process table `get`                | < 5 µs   |

Benchmarks live in `crates/avix-core/benches/`. Run with `cargo bench`.

---

## Key Architecture Decisions (ADRs)

These decisions are final. Do not re-open them without a compelling reason.

**ADR-01 — Tools are the security boundary.**
A separate coarse-grained capabilities layer is redundant when tools already represent
fine-grained permissions. `CapabilityToken.granted_tools` is the single source of truth.

**ADR-02 — llm.svc owns all inference.**
`RuntimeExecutor` never calls provider APIs directly. All AI calls go through
`llm.svc` via IPC. This isolates credentials, enables routing, and centralises observability.

**ADR-03 — Tool names use `/`, wire uses `__`.**
`fs/read` is the Avix name. `fs__read` is what appears on the wire to providers.
`RuntimeExecutor` always uses the unmangled name. Adapters translate at the boundary.
`ToolName::parse` rejects any name containing `__`.

**ADR-04 — Category 2 tools are registered at spawn, not hard-coded.**
`agent/spawn`, `pipe/open`, `cap/request-tool`, etc. are registered by `RuntimeExecutor`
via `ipc.tool-add` at spawn time and removed via `ipc.tool-remove` at exit. This means
the tool list the LLM sees always reflects the agent's actual runtime grants.

**ADR-05 — Fresh IPC connection per call.**
No connection multiplexing. No persistent channels. Every tool call opens a fresh
connection to `router.svc`, dispatches, and closes. This eliminates ordering bugs and
makes concurrency reasoning trivial.

**ADR-06 — Secrets are kernel-injected, never VFS-readable.**
`/secrets/` paths always return `EPERM` on VFS read. Secrets are decrypted by the kernel
and injected into the agent's environment at spawn. Agents never hold raw secret values.

**ADR-07 — ApprovalToken is single-use, atomically consumed.**
HIL escalation mints one `ApprovalToken` per event, broadcast to all `human_channel`
tools simultaneously. The first valid response atomically invalidates all others.
Subsequent consume attempts return `EUSED`.

**ADR-08 — auth.conf-first bootstrap.**
`avix start` aborts immediately if `auth.conf` does not exist. There is no fallback
mode, no default credentials, no "first run" wizard inside the kernel. The only path to
a running system is `avix config init` → `avix start`.

---

## Common Mistakes to Avoid

| Mistake | Correct approach |
|---|---|
| Calling provider API from `RuntimeExecutor` | Call `llm/complete` via IPC → `llm.svc` handles it |
| Using `"__"` in a tool name | Tool names use `/`; `__` is only on the wire |
| Reading from `/secrets/` via VFS | Secrets are env-injected at spawn only |
| Storing session state in `/users/` | Sessions live in `/proc/` — ephemeral only |
| Writing to `/proc/` from user code | `/proc/` is kernel-owned — read-only from agents |
| Using `credential.type: none` | Does not exist — use `api_key` or `password` |
| Hard-coding socket paths | Use `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK` env vars |
| Holding `AVIX_MASTER_KEY` after Phase 2 | Zero the env var immediately after reading |
| Registering Category 2 tools in a service | Register them in `RuntimeExecutor` at spawn |
| Calling LLM from kernel code | Kernel calls are deterministic; LLM is stateless |
| Writing `service.yaml` | `service.yaml` uses **YAML** format — see `docs/architecture/07-services.md` |
| Constructing `ServiceSpawnRequest { name, binary }` literals | Use `ServiceSpawnRequest::simple(name, binary)` or `ServiceSpawnRequest::from_unit(&unit)` |
| Injecting `_caller` unconditionally | Only inject when `ServiceRegistry::is_caller_scoped(svc)` returns true |
| Writing invocation records via VFS | Use `LocalProvider` directly (kernel is trusted) — VFS ACL layer would block it |
| Confusing sessions with invocations | Sessions are ephemeral (`/proc/`); invocations are persistent (`users/<u>/agents/`) |
| Using `service.unit` instead of `service.yaml` | Services use `service.yaml` (YAML), not TOML |

---

## Running the Project

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Check coverage (target: 95%+)
cargo tarpaulin --workspace --out Html --output-dir coverage/

# Run benchmarks
cargo bench

# Run clippy (must be zero warnings)
cargo clippy --workspace -- -D warnings

# Initialise config (first-time setup)
./target/debug/avix config init \
  --root ~/avix-data \
  --user alice \
  --role admin \
  --credential-type api_key \
  --mode cli

# Start the runtime
AVIX_MASTER_KEY=<your-key> ./target/debug/avix start --root ~/avix-data

# Check LLM provider status
./target/debug/avix llm status
```

---

## Development Plans

Active development plans live in `docs/dev_plans/`. These are gap analyses and focused
implementation tasks with TDD test code, implementation guidance, and success criteria.

```
docs/dev_plans/
├── README.md                          ← Overview of the dev_plans folder
├── fs-gap-A-bootstrap-vfs-init.md     ← Phase 1 VFS skeleton (proc/, kernel/ trees)
├── fs-gap-B-agent-spawn-vfs-writes.md ← /proc/<pid>/status.yaml + resolved.yaml at spawn
├── fs-gap-C-config-init-completeness.md ← config init writes all 6 /etc/avix/ files
├── fs-gap-D-vfs-write-protection.md   ← VfsPath::is_agent_writable() + syscall enforcement
├── fs-gap-E-mount-system.md           ← Mount system design (deferred to v0.2)
└── fs-gap-F-session-vfs-manifest.md   ← SessionStore writes /proc/users/<u>/sessions/
```

Files in `docs/dev_plans/` are considered temporary working documents and may be removed
once the work is complete and incorporated into `docs/architecture/`.

### Completed Plan Sets

**svc-gaps A–H** (service authoring) — fully implemented and incorporated into
`docs/architecture/07-services.md`. Plan files can be removed.

| Gap | What was built |
|-----|---------------|
| svc-gap-A | `ServiceUnit` TOML parser, `InstallReceipt`, `parse_duration` |
| svc-gap-B | `ServiceProcess` spawn + env injection, `ServiceStatus`, `discover_installed` |
| svc-gap-C | `ToolDescriptor`, `ToolScanner`, wire into `handle_ipc_register` |
| svc-gap-D | `ServiceInstaller` 7-step pipeline, `sys/install` syscall handler |
| svc-gap-E | `avix service install/list/status/start/stop/restart/uninstall/logs` CLI |
| svc-gap-F | `ipc.tool-add` / `ipc.tool-remove` typed wire params + `drain` semantics |
| svc-gap-G | `CallerInfo`, `caller_scoped` in `ServiceRecord` + `ServiceRegistry`, dispatcher injection |
| svc-gap-H | `ServiceWatchdog`, `SecretStore` (disk-backed), `kernel/secret/get`, `avix secret` CLI |

**Agent Persistence** — fully implemented and documented in `docs/architecture/14-agent-persistence.md`.

| Component | What was built |
|-----------|---------------|
| `ManifestScanner` | Scans `/bin/` + `/users/<u>/bin/` for installed agents; system wins collisions |
| `InvocationStore` | redb + LocalProvider; YAML summary + JSONL conversation per invocation |
| `ProcHandler` extension | `spawn()` creates record, `abort_agent()` finalizes Killed, 3 new list/get methods |
| `KernelIpcServer` extension | `kernel/proc/list-installed`, `kernel/proc/invocation-list`, `kernel/proc/invocation-get` |
| `RuntimeExecutor` extension | `shutdown_with_status()` flushes conversation + finalizes record on exit |
| ATP gateway | `proc/list-installed`, `proc/invocation-list`, `proc/invocation-get` forwarded via `ipc_forward` |
| CLI | `avix agent catalog`, `avix agent history [--agent]`, `avix agent show <id>` |
| TUI | Catalog tab (Tab key), `:catalog` command, `UpdateCatalog` / `SwitchTab` actions |
| GUI (`avix-app`) | CatalogPage (browse + spawn), HistoryPage (table + conversation drawer), 3 new Tauri commands |
