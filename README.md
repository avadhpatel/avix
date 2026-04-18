# Avix — Agent Operating System

Avix is an agent OS modelled on Unix/Linux primitives. Agents run as processes with
PIDs, the LLM acts as the CPU, and familiar OS abstractions — filesystem, signals, IPC,
capabilities — are applied to agentic concepts.

```
┌─────────────────────────────────────────────────────────────────────┐
│  Human / Client (ATP over WebSocket)                                │
│           ↓                                                         │
│   gateway.svc  ←→  auth.svc                                         │
│           ↓                                                         │
│      router.svc  (all tool calls)                                   │
│     ↙    ↓    ↘                                                     │
│  memfs  llm.svc  exec.svc  mcp-bridge  [installed services...]      │
│           ↑                                                         │
│   RuntimeExecutor  ←→  LLM (stateless, like a CPU)                 │
│      (the actual process — owns state, enforces policy)             │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### Prerequisites

- Rust 1.78+ (`rustup update stable`)
- `cargo-tarpaulin` for coverage: `cargo install cargo-tarpaulin --locked`

### Build

```bash
cargo build --workspace
```

### Daemon Setup

1. Init config (writes `auth.conf`, prints the generated API key):
   ```bash
   ./target/debug/avix server config init \
     --root ~/avix-data \
     --user alice \
     --role admin \
     --credential-type api_key \
     --mode cli
   ```

2. Start the daemon (ATP WebSocket on port 9142):
   ```bash
   export AVIX_MASTER_KEY=<your-32-byte-hex-key>
   ./target/debug/avix server start --root ~/avix-data
   ```

3. Connect and interact:
   ```bash
   export AVIX_API_KEY=<api-key-from-init>

   ./target/debug/avix client connect           # verify connectivity
   ./target/debug/avix client agent list        # list running agents
   ./target/debug/avix client agent spawn researcher \
     --goal "Research Q3 earnings"
   ./target/debug/avix client agent catalog     # list installed agents
   ./target/debug/avix client agent history     # past invocations
   ./target/debug/avix client agent show <id>   # detail + conversation
   ```

4. GUI dev:
   ```bash
   cd crates/avix-app
   npm install
   tauri dev  # auto-connects to localhost:9142
   ```

---

## ATP Manual Testing (websocat)

Test ATP directly against the daemon (port 9142 by default):

### 1. Login

```bash
curl -X POST http://localhost:9142/atp/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"identity":"alice","credential":"<api_key>"}'
```

### 2. WS Connect + Interact

```bash
websocat "ws://localhost:9142/atp" \
  -H "Authorization: Bearer <token_from_login>" \
  --interactive
```

In the websocat shell:
```
{"type":"subscribe","events":["*"]}
{"type":"cmd","id":"req-1","token":"<token>","domain":"proc","op":"spawn","body":{"agent":"researcher","goal":"Hello world"}}
```

---

## Architecture

### The Core Insight

The LLM is **stateless** — like a CPU executing instructions. The `RuntimeExecutor` is
the **process** — stateful, owns the conversation context, enforces capability policy,
and manages the full tool dispatch loop. Services are traditional deterministic software.
The capability token is the file descriptor table.

### Linux ↔ Avix Mapping

| Linux             | Avix                                          |
|-------------------|-----------------------------------------------|
| Kernel / PID 1    | `avix` runtime binary + `kernel.agent`        |
| Process           | Agent (LLM loop + `RuntimeExecutor`)          |
| Filesystem        | MemFS — in-memory VFS, driver-swappable       |
| Syscall           | `/tools/kernel/**` — 32 calls, 6 domains      |
| Shared library    | Service exposing tools at `/tools/<ns>/`      |
| IPC / socket      | `router.svc` + local sockets at `/run/avix/`  |
| Capability        | HMAC-signed `CapabilityToken`                 |
| Signal            | `SIGPAUSE`, `SIGRESUME`, `SIGESCALATE`, …     |
| cgroup            | Capability token scope                        |
| /proc             | `/proc/<pid>/status.yaml`                     |
| /etc/passwd       | `/etc/avix/users.yaml`                        |
| /etc/group        | `/etc/avix/crews.yaml`                        |
| sudoers           | `auth.conf` + `kernel/cap/policy`             |

### Two Communication Layers

```
EXTERNAL  —  clients ↔ Avix          INTERNAL  —  inside Avix
────────────────────────────          ──────────────────────────────
ATP over WebSocket (TLS)              JSON-RPC 2.0 over local sockets
Human users, apps, tooling            Services, agents, kernel
Authenticated via ATPToken            Authenticated via CapabilityToken
gateway.svc is the boundary           router.svc is the backbone
Long-lived, reconnectable             Fresh connection per call
```

### Filesystem Trees

```
/proc/          Ephemeral — per-agent runtime state (lost on reboot)
/kernel/        Ephemeral — system defaults and limits
/bin/           Persistent system — system-installed agents (all users)
/etc/avix/      Persistent system — configuration
/secrets/       Persistent — AES-256-GCM encrypted credentials
                  (never readable via VFS — kernel-injected only)
/users/         Persistent user — operator workspaces
  └── <username>/bin/       User-installed agents (that user only)
  └── <username>/agents/    Invocation records + conversation history
/services/      Persistent — service account workspaces
/crews/         Persistent — crew shared spaces
```

### LLM Tool Exposure Model

Every Avix feature is exposed to the LLM as a **tool** — never as raw IPC, signals, or
capability tokens.

| Category | Examples | How it works |
|---|---|---|
| **1 — Direct** | `fs/read`, `llm/complete`, `exec/python` | LLM calls → RuntimeExecutor validates + dispatches |
| **2 — Avix Behaviour** | `agent/spawn`, `pipe/open`, `cap/escalate` | Registered at spawn by RuntimeExecutor; translates to kernel syscall |
| **3 — Transparent** | HIL gating, token renewal, snapshot triggers | LLM never sees these; RuntimeExecutor handles automatically |

### Multi-Modality LLM

All AI inference goes through `llm.svc` — agents never call providers directly.

| Modality       | Tool                  | Output                      |
|----------------|-----------------------|-----------------------------|
| Text           | `llm/complete`        | Text content blocks         |
| Image          | `llm/generate-image`  | VFS path (scratch dir)      |
| Speech         | `llm/generate-speech` | VFS path (scratch dir)      |
| Transcription  | `llm/transcribe`      | Text                        |
| Embedding      | `llm/embed`           | Float vector                |

Supported providers: Anthropic, OpenAI, Ollama, Stability AI, ElevenLabs.

---

## Clients

* **Daemon**: `avix server start --root <dir> [--port 9142]` — ATP WS gateway + services + kernel.agent
* **CLI**: `avix client <subcommand>` — full control over agents, sessions, services, secrets, HIL
* **TUI**: `avix client tui` — fullscreen dashboard: Running tab (agents/output/events/notifs/HIL) + Catalog tab (`Tab` to switch). Commands: `:spawn`/`:kill`/`:logs`/`:catalog`
* **GUI**: `cd crates/avix-app && tauri dev` — sidebar app with Agent threads, Catalog page (browse + spawn), History page (invocation table + conversation drawer), Services, Tools

All share `avix-client-core` ATP lib.

### Server Commands

```bash
# Initialize config (must run once before starting)
avix server config init \
  --root ~/avix-data \
  --user alice \
  --role admin \
  --credential-type api_key \
  --mode cli

# Start the daemon (ATP WebSocket on port 9142)
avix server start --root ~/avix-data [--port 9142]

# Run a single agent directly (no daemon required)
avix server run --goal "Summarise Q3 earnings" [--name researcher]
```

### Client — Agent Commands

```bash
avix client agent list                          # list running agents
avix client agent spawn researcher \
  --goal "Research Q3 earnings"                 # spawn an agent
avix client agent kill <pid>                    # kill an agent by PID
avix client agent catalog [--username]          # list installed agents
avix client agent history [--agent] [--username] # past invocations
avix client agent show <id>                     # detail + conversation
avix client agent install ./my-agent.tar.xz    # from local archive
avix client agent install https://...          # from URL
avix client agent install github:owner/repo/agent  # from GitHub Releases
avix client agent uninstall my-agent [--scope user|system]

# All agent commands accept --json for machine-readable output
```

### Client — Session Commands

Sessions group related invocations. Ownership is enforced: users access only their own
sessions; operator and admin roles bypass the check.

```bash
avix client session list [--username] [--status idle|running|completed|failed]
avix client session show <session_id>
avix client session resume <session_id> [--input "continue with X"]
avix client session delete <session_id> [--force]
```

### Client — Other Commands

```bash
# Connectivity
avix client connect                      # test connectivity (reads config.yaml)
avix client tui                          # launch TUI dashboard
avix client logs [--follow]              # tail server logs
avix client atp shell                    # interactive ATP shell (REPL)

# HIL (Human-in-the-Loop)
avix client hil list                     # pending HIL requests
avix client hil approve <hil_id>
avix client hil deny <hil_id>

# Services
avix client service list
avix client service status <name>
avix client service start <name>
avix client service stop <name>
avix client service restart <name>
avix client service uninstall <name> [--force]

# Secrets (requires AVIX_MASTER_KEY in env)
avix client secret set <key> <value> --for-service <svc>
avix client secret set <key> <value> --for-user <user>
avix client secret list [--for-service <svc>] [--for-user <user>]
avix client secret delete <key> [--for-service <svc>] [--for-user <user>]
```

### Package Commands (offline — no server required)

```bash
avix package new my-agent --type agent          # scaffold new agent
avix package new my-svc --type service          # scaffold new service
avix package validate ./my-agent                # validate package structure
avix package build ./my-agent --version v0.1.0  # build .tar.xz archive
avix package trust add <key> --name "AcmeCorp"  # add trusted signing key
avix package trust list
avix package trust remove <fingerprint>
```

---

## Repository Layout

```
avix/ (Cargo workspace)
├── Cargo.toml
├── crates/
│   ├── avix-client-core/    ← ATP protocol + shared state
│   ├── avix-core/           ← Runtime + kernel + VFS + IPC
│   ├── avix-cli/            ← CLI binary
│   ├── avix-app/            ← Tauri GUI (Rust backend + React/Vite frontend)
│   └── avix-docker/         ← Headless daemon
├── docs/architecture/       ← 00-14 docs
└── ...
```

## Development

### Run Tests

```bash
cargo test --workspace

# ATP WS E2E
cargo test -p avix-tests-integration
```

### Coverage (target: 95%+)

```bash
cargo tarpaulin --workspace --out Html --output-dir coverage/
open coverage/tarpaulin-report.html
```

### Linting

```bash
cargo clippy --workspace -- -D warnings   # must be zero warnings
cargo fmt --check                          # must be clean
```

### Benchmarks

```bash
cargo bench
```

Performance targets:

| Operation | Target |
|---|---|
| Boot to ready | < 700 ms |
| ATPToken validation | < 50 µs |
| VFS file read | < 50 µs |
| Tool registry lookup | < 5 µs |
| IPC frame encode + decode | < 10 µs |
| Tool name mangle | < 1 µs |

### TDD Workflow

Every change follows the same cycle:

1. Write the failing test
2. `cargo test --workspace` — confirm it fails
3. Implement the minimum code to make it pass
4. Refactor
5. `cargo clippy --workspace -- -D warnings && cargo fmt --check`
6. Commit

See `CLAUDE.md` for the full development convention reference.

---

## Deployment Modes

| Mode | Use case | gateway.bind | Master key source |
|---|---|---|---|
| `gui` | Desktop app | localhost | OS keychain (env) |
| `cli` | Developer workstation | localhost | Key file or env |
| `headless` | Docker / CI | 0.0.0.0 | Docker secret / env |
| `headless` | Remote server | 0.0.0.0 | AWS KMS / GCP KMS / Vault |

### Docker

```dockerfile
FROM avix:latest
ENV AVIX_MASTER_KEY=""
ENV AVIX_ADMIN_API_KEY=""
RUN avix server config init \
  --root /var/avix-data \
  --user avix-admin \
  --credential-type api_key \
  --api-key "$AVIX_ADMIN_API_KEY" \
  --master-key-source env \
  --mode headless \
  --non-interactive
CMD ["avix", "server", "start", "--root", "/var/avix-data"]
```

---

## Security Model

- **Credentials** — never stored in plaintext. API keys are HMAC-SHA256 hashed.
  Passwords use argon2id (`m=65536, t=3, p=4`).
- **Secrets** — AES-256-GCM at rest. VFS reads of `/secrets/` always return `EPERM`.
  Secrets are kernel-injected into agent environments at spawn only.
- **Capability tokens** — HMAC-signed, scoped to a specific set of tools. A child agent
  can never exceed its parent's permissions.
- **HIL (Human-in-Loop)** — configurable per-tool approval gates. `SIGPAUSE` freezes the
  agent; `SIGRESUME` unfreezes with the human decision injected as context.
- **Per-message `_caller` injection** — every inbound tool call to a service includes
  `_caller.pid`, `_caller.user`, and `_caller.token`. Services use this to scope
  per-user behaviour. Unauthorized calls never reach the service — the kernel enforces
  ACLs at dispatch time.
- **Session ownership** — users can only read and mutate their own sessions. Operator and
  admin roles bypass this check. Ownership is enforced at the kernel IPC layer, not just
  the gateway.

---

## Key Design Invariants

1. `auth.conf` must exist before `avix server start` — no setup mode inside core
2. `credential.type: none` does not exist — all auth is `api_key` or `password`
3. ATP (external) and IPC (internal) never cross the boundary
4. `llm.svc` owns all AI inference — `RuntimeExecutor` never calls providers directly
5. Kernel syscalls are deterministic — never LLM-decided
6. Tool names use `/`; wire uses `__`; no Avix name ever contains `__`
7. Secrets are kernel-injected only — never VFS-readable
8. Category 2 tools are registered at spawn and deregistered at exit
9. Fresh IPC connection per call — no persistent multiplexed channels
10. `ApprovalToken` is single-use — atomic first-responder-wins semantics

---

## Contributing

See `CONTRIBUTING.md` and `CLAUDE.md`. The project uses strict TDD with a 95%+ coverage
gate. All PRs must pass `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check`.

---

## License

MIT — see `LICENSE`.
