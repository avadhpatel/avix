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

### Initialise Configuration

Run once before first start. Generates `auth.conf` and prints your API key — store it
in your password manager:

```bash
./target/debug/avix config init \
  --root ~/avix-data \
  --user alice \
  --role admin \
  --credential-type api_key \
  --mode cli
```

### Start the Runtime

```bash
export AVIX_MASTER_KEY=<your-32-byte-key>
export AVIX_API_KEY=<the-key-printed-by-config-init>
./target/debug/avix start --root ~/avix-data
```

### Connect

```bash
# Check runtime status
./target/debug/avix status

# Spawn an agent
./target/debug/avix agent spawn \
  --agent researcher \
  --goal "Summarise the Q3 earnings report in /users/alice/workspace/q3.pdf"

# List running agents
./target/debug/avix agent list

# Check LLM provider health
./target/debug/avix llm status
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
/bin/           Persistent system — installed agents
/etc/avix/      Persistent system — configuration
/secrets/       Persistent — AES-256-GCM encrypted credentials
                  (never readable via VFS — kernel-injected only)
/users/         Persistent user — operator workspaces
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

## Repository Layout

```
avix/
├── CLAUDE.md                 ← AI assistant development instructions
├── README.md                 ← This file
├── Cargo.toml                ← Workspace root
│
├── crates/
│   ├── avix-core/            ← All logic as a library (tested here)
│   ├── avix-cli/             ← CLI binary (thin wrapper)
│   ├── avix-app/             ← Desktop app binary (thin wrapper)
│   └── avix-docker/          ← Headless Docker binary (thin wrapper)
│
├── docs/
│   ├── architecture/
│   │   ├── 00-overview.md
│   │   ├── 01-filesystem.md
│   │   ├── 02-bootstrap.md
│   │   ├── 03-ipc.md
│   │   ├── 04-atp.md
│   │   ├── 05-capabilities.md
│   │   ├── 06-agents.md
│   │   ├── 07-services.md
│   │   ├── 08-llm-service.md         ← LLM service spec
│   │   └── 09-runtime-executor-tools.md  ← Tool exposure model
│   ├── development/
│   │   ├── setup.md
│   │   ├── testing.md
│   │   ├── tdd-workflow.md
│   │   └── benchmarking.md
│   ├── user/
│   │   ├── quickstart.md
│   │   ├── installation.md
│   │   └── tutorial.md
│   └── dev_plans/            ← Active development plans and gap analyses
│
└── .github/
    └── workflows/
        ├── ci.yml
        └── coverage.yml
```

---

## Development

### Run Tests

```bash
cargo test --workspace
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

All performance targets must pass before the Day 29 milestone:

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
6. Commit: `day-NN: <description>`

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
RUN avix config init \
  --root /var/avix-data \
  --user avix-admin \
  --credential-type api_key \
  --api-key "$AVIX_ADMIN_API_KEY" \
  --master-key-source env \
  --mode headless \
  --non-interactive
CMD ["avix", "start", "--root", "/var/avix-data"]
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

---

## Key Design Invariants

1. `auth.conf` must exist before `avix start` — no setup mode inside core
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
