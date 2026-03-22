# Avix Architecture Reference (v3)

> **Purpose:** Authoritative development reference for the Avix agent operating system.
> **Scope:** Filesystem, schemas, deployment, bootstrap, kernel config, services, agents,
> IPC protocol, tool namespace, storage, security, ATP, and job system.
> **Supersedes:** Architecture Reference v2

-----

## Table of Contents

1. [Overview](#1-overview)
1. [Core Concepts](#2-core-concepts)
1. [Filesystem Layout](#3-filesystem-layout)
1. [YAML Schema Conventions](#4-yaml-schema-conventions)
1. [Schema Index](#5-schema-index)
1. [Bootstrap Sequence](#6-bootstrap-sequence)
1. [Deployment Modes](#7-deployment-modes)
1. [Initial Configuration — avix config init](#8-initial-configuration--avix-config-init)
1. [KernelConfig — /etc/avix/kernel.yaml](#9-kernelconfig--etcavixkernelyaml)
1. [Users — /etc/avix/users.yaml](#10-users--etcavixusersyaml)
1. [Crews — /etc/avix/crews.yaml](#11-crews--etcavixcrewsyaml)
1. [Services](#12-services)
1. [Agents](#13-agents)
1. [IPC Protocol](#14-ipc-protocol)
1. [Kernel Syscalls — /tools/kernel/](#15-kernel-syscalls--toolskernel)
1. [Tool Namespace — /tools/](#16-tool-namespace--tools)
1. [Storage Backends and Mount System](#17-storage-backends-and-mount-system)
1. [CapabilityToken](#18-capabilitytoken)
1. [ResourceRequest and ResourceResponse](#19-resourcerequest-and-resourceresponse)
1. [Signals](#20-signals)
1. [Pipes](#21-pipes)
1. [Snapshots](#22-snapshots)
1. [Crontab](#23-crontab)
1. [Defaults and Limits — Resolution Order](#24-defaults-and-limits--resolution-order)
1. [Resolved Config](#25-resolved-config)
1. [Session and Capability Model](#26-session-and-capability-model)
1. [Avix Terminal Protocol (ATP)](#27-avix-terminal-protocol-atp)
1. [exec.svc — Runtime Discovery](#28-execsvc--runtime-discovery)
1. [mcp-bridge.svc](#29-mcp-bridgesvc)
1. [jobs.svc — Long-Running Jobs](#30-jobssvc--long-running-jobs)
1. [Secrets Store](#31-secrets-store)
1. [Installation and Packaging](#32-installation-and-packaging)
1. [Unit File Format](#33-unit-file-format)
1. [Validation Rules](#34-validation-rules)
1. [Open Questions](#35-open-questions)

-----

## 1. Overview

Avix is an agent operating system modelled on Unix/Linux primitives. The design maps agentic concepts onto familiar OS abstractions:

|Linux concept   |Avix equivalent                                                                               |
|----------------|----------------------------------------------------------------------------------------------|
|Kernel / PID 1  |`avix` runtime binary + `kernel.agent`                                                        |
|Processes       |Agents (LLM conversation loops with a RuntimeExecutor)                                        |
|Filesystem      |MemFS — driver-swappable VFS                                                                  |
|Syscalls        |`/tools/kernel/**` — 32 calls across 6 domains                                                |
|Shared libraries|Services exposing tools at `/tools/<namespace>/`                                              |
|IPC / sockets   |`router.svc` + platform-native local sockets at `/run/avix/`                                  |
|Capabilities    |HMAC-signed capability tokens issued by `auth.svc`                                            |
|Signals         |`SIGSTART`, `SIGPAUSE`, `SIGRESUME`, `SIGKILL`, `SIGSTOP`, `SIGSAVE`, `SIGPIPE`, `SIGESCALATE`|
|cgroups         |Capability token scopes                                                                       |
|/proc           |`/proc/<pid>/status.yaml`, `/proc/<pid>/resolved.yaml`                                        |
|dmesg           |Panic ring buffer → `kernel/sys/boot-log`                                                     |
|systemd units   |`service.unit` and `agent.unit` files                                                         |
|initramfs       |Fused bootstrap phase (Phase 0–2)                                                             |
|pivot_root      |Re-exec handoff (the Pivot)                                                                   |
|Package manager |`avix service install` / `avix agent install` (also via ATP `sys.install`)                    |
|/etc/passwd     |`/etc/avix/users.yaml`                                                                        |
|/etc/group      |`/etc/avix/crews.yaml`                                                                        |
|sudoers         |`/etc/avix/auth.conf` + `kernel/cap/policy`                                                   |

**The load-bearing architectural insight:** The LLM is stateless — analogous to a CPU. The `RuntimeExecutor` is the actual process — analogous to an OS process with a file descriptor table. Services are traditional deterministic software written in any language. Agents are LLM conversation loops. The capability token system is the trust boundary between them.

**Key design decisions (v3):**

- Avix core always boots from pre-existing, valid `/etc/avix/` config files — there is no “setup mode” inside the core. Configuration is produced by `avix config init` before first start.
- All external client communication uses the Avix Terminal Protocol (ATP) over WebSocket. Internal communication uses the IPC protocol over platform-native local sockets.
- Services are language-agnostic host processes. Any language that can open a socket and speak JSON-RPC 2.0 can implement a service.
- `credential.type: none` and `auto_session` are removed. All callers authenticate via API key or password.
- IPC transport is `local-ipc` — Unix domain sockets on Linux/macOS, Named Pipes on Windows. The kernel resolves the platform path; config uses logical names only.

-----

## 2. Core Concepts

### What is an Agent vs a Service?

The test: *Could a deterministic program with fixed rules solve this reliably?*

- **YES → Service.** File I/O, auth, routing, logging, scheduling, code execution, MCP adapting. No LLM required. Always available. Written in any language.
- **NO → Agent.** Interpreting ambiguous intent, multi-step planning, synthesising context, deciding which tool to call. Requires LLM.

### The LLM-as-CPU Analogy

```
LLM inference call   =   CPU instruction execution  (stateless, repeatable)
RuntimeExecutor      =   OS process                 (stateful, owns context)
Capability token     =   File descriptor table      (scoped access list)
/tools/**            =   System call table          (stable API surface)
```

### Two Communication Layers

```
EXTERNAL — clients ↔ Avix             INTERNAL — processes inside Avix
────────────────────────────          ─────────────────────────────────
ATP over WebSocket (TLS)              JSON-RPC 2.0 over local IPC sockets
Human users, apps, tooling            Services, agents, kernel
Authenticated via ATPToken            Authenticated via CapabilityToken / SvcToken
gateway.svc is the boundary           router.svc is the backbone
Long-lived, reconnectable             Fast, local, synchronous/async
```

ATP never goes inside the system. `gateway.svc` translates ATP commands into internal IPC calls. The internal world never speaks ATP.

### Avix is LLM-optional

The service tier runs with zero LLM dependency. `kernel.agent` activates only when `model.conf` is present.

-----

## 3. Filesystem Layout

The Avix filesystem is divided into four trees based on ownership and lifetime. **Ownership is encoded in location** — a file in the wrong tree is a bug.

### 3.1 Filesystem Trees

```
┌─────────────────────────────────────────────────────────────────┐
│  EPHEMERAL — Owner: Kernel — Lifetime: Lost on reboot           │
│                                                                 │
│  /proc/      per-agent, per-user, per-service runtime state     │
│  /kernel/    system-wide VFS (defaults, limits)                 │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SYSTEM — Owner: root — Survives reboot            │
│                                                                 │
│  /bin/       system-installed agents                            │
│  /etc/avix/  system configuration                               │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — SECRETS — Kernel-mediated — Not portable          │
│                                                                 │
│  /secrets/<username>/    encrypted credential store per user    │
│  /secrets/services/<n>/  encrypted credential store per service │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  PERSISTENT — USER/OPERATOR — Portable — Freely exportable      │
│                                                                 │
│  /users/<username>/        human operator workspaces            │
│  /services/<svcname>/      service account workspaces           │
│  /crews/<crew-name>/       crew shared spaces                   │
└─────────────────────────────────────────────────────────────────┘
```

**Hard rules:**

- The kernel never writes into user-owned trees (`/users/`, `/services/`, `/crews/`)
- Users and agents never write into ephemeral or system trees
- Secrets in `/secrets/` are never readable via the VFS — only injectable by the kernel
- Sessions live in `/proc/` — they are runtime state, not user data

### 3.2 Disk Layout — AVIX_ROOT

All persistent trees live under a single `AVIX_ROOT` directory on the host. This is the only path Avix needs to know at boot. Avix derives all internal VFS paths from it. Individual subtrees can be overridden via `fstab.yaml`.

```
AVIX_ROOT/                 (e.g. ~/avix-data or /var/avix-data)
├── etc/                   → VFS /etc/avix/
│   ├── auth.conf          (chmod 600 — contains credential hashes)
│   ├── kernel.yaml        (chmod 600 — contains masterKey source config)
│   ├── boot.conf
│   ├── users.yaml
│   ├── crews.yaml
│   ├── crontab.yaml
│   └── fstab.yaml
├── bin/                   → VFS /bin/
├── services/              → VFS /services/
│   └── <svcname>/
│       ├── service.unit
│       ├── bin/
│       ├── tools/
│       ├── workspace/
│       └── .install.json  (install receipt)
├── users/                 → VFS /users/
│   └── <username>/
│       ├── workspace/
│       ├── snapshots/
│       ├── defaults.yaml
│       └── limits.yaml
├── crews/                 → VFS /crews/
├── secrets/               → VFS /secrets/ (AES-256-GCM blobs, chmod 700)
└── logs/                  → /var/log/avix/
```

**File permissions set by installer (not Avix core):**

|Path                        |Mode|Reason                         |
|----------------------------|----|-------------------------------|
|`AVIX_ROOT/etc/`            |700 |Only avix process user can read|
|`AVIX_ROOT/etc/auth.conf`   |600 |Credential hashes              |
|`AVIX_ROOT/etc/kernel.yaml` |600 |Master key source              |
|`AVIX_ROOT/secrets/`        |700 |Kernel-only tree               |
|`AVIX_ROOT/secrets/**/*.enc`|600 |Encrypted blobs                |
|`AVIX_ROOT/users/`          |755 |No secrets — freely readable   |
|`AVIX_ROOT/services/`       |755 |Service assets                 |

### 3.3 Full VFS Directory Reference

#### Ephemeral Tree

```
/proc/
├── <pid>/
│   ├── status.yaml
│   ├── resolved.yaml
│   ├── pipes/<pipe-id>.yaml
│   └── hil-queue/<request-id>.yaml
├── users/<username>/
│   ├── status.yaml
│   ├── sessions/<session-id>.yaml
│   ├── logs/
│   └── resolved/<kind>.yaml
├── services/<svcname>/
│   ├── status.yaml
│   └── logs/
├── gateway/
│   ├── connections.yaml      ← live ATP connection registry
│   └── subscriptions.yaml    ← per-session event subscriptions
└── spawn-errors/<request-id>.yaml

/kernel/
├── defaults/<kind>.yaml
└── limits/<kind>.yaml
```

#### Persistent System Tree

```
/bin/<agent>/
│   ├── manifest.yaml
│   └── ...

/etc/avix/
├── auth.conf
├── kernel.yaml
├── boot.conf
├── users.yaml
├── crews.yaml
├── crontab.yaml
└── fstab.yaml
```

#### Persistent Secrets Tree

No path under `/secrets/` is ever readable via a VFS `read` call.

```
/secrets/
├── <username>/<secret-name>.enc
└── services/<svcname>/<secret-name>.enc
```

#### Persistent User/Operator Tree

```
/users/<username>/
├── bin/<agent>/manifest.yaml
├── defaults.yaml
├── limits.yaml
├── workspace/
└── snapshots/<agent>-<timestamp>.yaml

/services/<svcname>/
├── bin/
├── defaults.yaml
├── limits.yaml
├── workspace/
├── snapshots/
└── .install.json

/crews/<crew-name>/
├── defaults.yaml
├── limits.yaml
└── shared/
```

#### Runtime Sockets

```
/run/avix/
├── kernel.sock              ← ResourceRequests, KernelSyscalls
├── router.sock              ← all tool calls route here first
├── auth.sock
├── memfs.sock
├── agents/<pid>.sock        ← signal delivery TO agents
└── services/<name>.sock     ← per-service tool call endpoints
```

-----

## 4. YAML Schema Conventions

All configuration files use YAML with Kubernetes-style structure.

|Field       |Rule                                         |
|------------|---------------------------------------------|
|`apiVersion`|Always `avix/v1`                             |
|`kind`      |PascalCase resource type                     |
|`metadata`  |Provenance fields (name, version, timestamps)|
|`spec`      |**Required** on authored files               |
|`status`    |Kernel-written runtime state — read-only     |
|`limits`    |Kernel-owned bounds — read-only              |
|`defaults`  |Fallback values                              |
|`resolved`  |Kernel-derived merge — never authored        |

- All timestamps: ISO 8601 with timezone `2026-03-15T07:38:00-05:00`
- All durations in seconds unless noted: `timeoutSec: 300`
- Unknown fields are rejected by the validator

-----

## 5. Schema Index

|# |Kind            |Location                                              |Direction               |
|--|----------------|------------------------------------------------------|------------------------|
|1 |AgentManifest   |`/bin/<agent>/manifest.yaml`                          |Config (static)         |
|2 |AgentStatus     |`/proc/<pid>/status.yaml`                             |Status (dynamic)        |
|3 |Users           |`/etc/avix/users.yaml`                                |Config (static)         |
|4 |Crews           |`/etc/avix/crews.yaml`                                |Config (static)         |
|5 |KernelConfig    |`/etc/avix/kernel.yaml`                               |Config (static)         |
|6 |AuthConfig      |`/etc/avix/auth.conf`                                 |Config (static)         |
|7 |CapabilityToken |Issued by kernel at spawn                             |Runtime (issued)        |
|8 |ATPToken        |Issued by auth.svc on login                           |Runtime (issued)        |
|9 |ResourceRequest |Agent → Kernel (IPC)                                  |Runtime (request)       |
|10|ResourceResponse|Kernel → Agent (IPC)                                  |Runtime (response)      |
|11|Signal          |Kernel ↔ Agent (IPC event)                            |Runtime (event)         |
|12|HilRequest      |`/proc/<pid>/hil-queue/<hil-id>.yaml`                 |Runtime (ephemeral)     |
|13|Pipe            |`/proc/<pid>/pipes/<id>.yaml`                         |Runtime (channel)       |
|14|Crontab         |`/etc/avix/crontab.yaml`                              |Config (static)         |
|15|Snapshot        |`/users/<username>/snapshots/`                        |Persistence             |
|16|SessionManifest |`/proc/users/<username>/sessions/<sid>.yaml`          |Status (ephemeral)      |
|17|Defaults        |`/kernel/defaults/`, `/users/<username>/defaults.yaml`|Config (layered)        |
|18|Limits          |`/kernel/limits/`, `/users/<username>/limits.yaml`    |Runtime (kernel-owned)  |
|19|Resolved        |`/proc/<pid>/resolved.yaml`                           |Runtime (kernel-derived)|

-----

## 6. Bootstrap Sequence

Avix boots in five phases. Phases 0–2 run inside a single process (fused). The Pivot splits them into sidecars. Phase 4 starts the service and agent layers.

**Prerequisite:** `/etc/avix/auth.conf` must exist and be valid before `avix start`. Avix refuses to boot without it. Configuration is produced by `avix config init` (see §8).

### Environment Variables

```bash
AVIX_ROOT=/var/avix-data       # host FS path — all config derived from here
AVIX_IPC_DIR=/run/avix         # platform-resolved socket directory
AVIX_MASTER_KEY=<key>          # secrets master key (env source) — zeroed after Phase 2
AVIX_LOG_LEVEL=info
```

### Phase 0 — Runtime Self-Init

- Parse env vars / CLI flags
- Allocate process table (PID 0 = runtime)
- Wire signal bus
- Initialise panic ring buffer
- Verify `AVIX_ROOT` readable — exit 1 on failure

### Phase 1 — Fused Bootstrap

- **memfs** (local driver only) — opens `AVIX_ROOT` on host FS, mounts as `/`
- **router** (in-process socket) — creates `AVIX_IPC_DIR/router.sock`
- **auth** (default caps only) — PID 0 gets `auth:admin`, no `auth.conf` yet
- **logger** — opens `/var/log/avix/boot.log`

### Phase 2 — Read Config

- Read `AVIX_ROOT/etc/boot.conf`
- Read `AVIX_ROOT/etc/kernel.yaml`
- Hot-swap memfs driver if `storage.driver ≠ local`
- Read `AVIX_ROOT/etc/auth.conf` — replace bootstrap auth with full policy
- Load secrets master key from configured source → held in memory only; env var zeroed
- Validate — halt with structured error on any failure

### The Pivot — Re-exec Handoff

```
avix runtime forks:
  router.svc  → PID 2  (inherits router.sock fd)
  auth.svc    → PID 3  (inherits serialised auth state)
  logger.svc  → PID 4  (inherits open log fd)
  memfs.svc   → PID 5  (inherits all open file handles)

runtime becomes supervisor (PID 1):
  owns: process table, signal dispatch, re-exec of failed built-ins
```

### Phase 4 — Service Boot

```
Ring-1 (built-in, checksum-verified):
  1. router.svc, auth.svc, memfs.svc, logger.svc  (already live from Pivot)

Ring-2 (built-in, checksum-verified):
  2. watcher.svc
  3. scheduler.svc
  4. tool-registry.svc
  5. exec.svc
  6. mcp-bridge.svc
  7. gateway.svc
  8. gui.svc           (if mode=gui)
  9. shell.svc         (if shell=true)

Ring-3 (installed services — signature-verified):
  10. Read /services/*/service.unit
      Sort by [after:] dependency order
      Spawn each as host OS process
      kernel/ipc/register each
      tool-registry.svc rescan after all up

Agents:
  11. kernel.agent     (always — LLM-optional)
  12. planner.agent, executor.agent, memory.agent, observer.agent
      (spawned by kernel.agent when LLM available)
```

Installed services fail independently. A broken service does not prevent boot. Kernel logs the failure and marks the service `unavailable`.

-----

## 7. Deployment Modes

Mode is set by `boot.conf: [runtime] mode`.

### Mode 1 — Desktop App / GUI

```toml
[runtime]
avix_root = ~/avix-data
mode      = gui

[ui]
port  = 7700
shell = true

[sessions]
ttl = 24h

[gateway]
bind       = localhost
user_port  = 7700
admin_port = 7701
```

The desktop app (Electron/native) manages credentials. At launch it:

1. Reads the API key from the OS keychain
1. Derives the master key (machine_id + app_bundle_id → HKDF)
1. Spawns `avix start --root ~/avix-data` with `AVIX_MASTER_KEY` in env
1. Connects to ATP using the keychain-held API key

The user never sees a password prompt. The “passwordless feel” is achieved through OS keychain — not an unsecured Avix boot mode.

### Mode 2 — CLI

```toml
[runtime]
avix_root = ~/avix-data
mode      = cli

[ui]
shell = true

[sessions]
ttl = 8h

[gateway]
bind       = localhost
user_port  = 7700
admin_port = 7701
```

The `avix` CLI reads `AVIX_API_KEY` from env or `--api-key` flag. The key is stored in the user’s password manager or shell profile.

### Mode 3 — Docker Headless

```toml
[runtime]
avix_root = /var/avix-data
mode      = headless

[ui]
shell = false

[sessions]
ttl = 1h

[gateway]
bind       = 0.0.0.0
user_port  = 7700
admin_port = 7701
```

Config files are generated before container start. API key and master key are injected via Docker secrets / env vars.

### Mode 4 — Remote Server

Same as Docker but deployed directly on a host. Uses KMS or Vault for the master key. `ip_allowlist` on credentials restricts access to known CIDR ranges.

### Mode Diff Table

|Field            |Desktop App      |CLI                    |Docker             |Remote          |
|-----------------|-----------------|-----------------------|-------------------|----------------|
|`avix_root`      |`~/avix-data`    |`~/avix-data`          |`/var/avix-data`   |`/var/avix-data`|
|`mode`           |`gui`            |`cli`                  |`headless`         |`headless`      |
|`gateway.bind`   |`localhost`      |`localhost`            |`0.0.0.0`          |`0.0.0.0`       |
|`ttl`            |`24h`            |`8h`                   |`1h`               |`1h`            |
|Master key source|OS keychain (env)|key-file               |Docker secret (env)|KMS             |
|Credential type  |`api_key`        |`api_key` or `password`|`api_key`          |`api_key`       |

-----

## 8. Initial Configuration — avix config init

Avix core never configures itself. `avix config init` is a pre-boot file generator run by the installer (desktop app, Docker entrypoint, provisioning script) before `avix start`.

### What it does

1. Generates an API key: `sk-avix-<32 base62 chars>` (if not provided)
1. Computes `hmac-sha256(api_key)` — stores hash, never plaintext
1. Writes `AVIX_ROOT/etc/auth.conf` with the hash
1. Writes `AVIX_ROOT/etc/boot.conf`, `users.yaml`, `kernel.yaml`
1. Sets file permissions (`chmod 600` on sensitive files)
1. Prints the API key once to stdout — never stored

The plaintext API key is the caller’s responsibility to store (OS keychain, password manager, Docker secret).

### CLI Usage

```bash
# Desktop app / CLI — API key generated, printed once
avix config init \
  --root ~/avix-data \
  --user alice \
  --role admin \
  --credential-type api_key \
  --master-key-source key-file \
  --master-key-file ~/.config/avix/master.key \
  --mode gui

# Docker — non-interactive, env-driven
avix config init \
  --root /var/avix-data \
  --user avix-admin \
  --credential-type api_key \
  --api-key "$AVIX_ADMIN_API_KEY" \
  --master-key-source env \
  --mode headless \
  --non-interactive

# Remote server with AWS KMS
avix config init \
  --root /var/avix-data \
  --user avix-remote \
  --credential-type api_key \
  --master-key-source kms-aws \
  --kms-key-id arn:aws:kms:us-east-1:... \
  --ip-allowlist 10.0.0.0/8 \
  --non-interactive
```

### auth.conf Schema

```yaml
apiVersion: avix/v1
kind: AuthConfig

policy:
  session_ttl: 8h
  require_tls: true
  failed_auth_lockout_count: 5
  failed_auth_lockout_ttl: 15m
  token_refresh_window: 5m

identities:
  - name: alice
    uid: 1001
    role: admin                       # guest | user | operator | admin
    credential:
      type: api_key                   # password | api_key
      key_hash: hmac-sha256:$...      # for api_key: HMAC-SHA256 hash
      # hash: argon2id:$argon2id$...  # for password: argon2id hash
      ip_allowlist: []                # empty = allow all; or CIDR list
```

**Credential types:**

|Type      |Storage         |Plaintext on disk?|Remote access|
|----------|----------------|------------------|-------------|
|`password`|argon2id hash   |Never             |Yes          |
|`api_key` |HMAC-SHA256 hash|Never             |Yes          |

Argon2id parameters: `m=65536, t=3, p=4` (OWASP-recommended minimums).
API key format: `sk-avix-<32 base62 chars>` (~190 bits entropy).

### Idempotency

`avix config init` without `--force` refuses to overwrite existing `auth.conf`. Safe to run in container entrypoints — subsequent runs are no-ops.

-----

## 9. KernelConfig — /etc/avix/kernel.yaml

```yaml
apiVersion: avix/v1
kind: KernelConfig
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  scheduler:
    algorithm: priority_deadline   # priority_deadline | round_robin | fifo
    tickMs: 100
    preemption: true
    maxConcurrentAgents: 50

  memory:
    defaultContextLimit: 200000
    evictionPolicy: lru_salience
    maxEpisodicRetentionDays: 30
    sharedMemoryPath: /shared/

  ipc:
    transport: local-ipc           # platform-resolved: unix-socket on Linux/macOS,
                                   # named-pipe on Windows — requires restart to change
    socket_name: kernel            # logical name; Avix resolves OS path:
                                   #   Linux/macOS: /run/avix/kernel.sock
                                   #   Windows:     \\.\pipe\avix-kernel
    maxMessageBytes: 65536
    timeoutMs: 5000

  safety:
    policyEngine: enabled
    maxToolChainLength: 10

    # HIL policy
    hil_timeout: 10m             # auto-deny pending HIL requests after this duration
    hilOnEscalation: true        # pause + surface to human when agent sends SIGESCALATE

    # tool_call_approval: tools that always require human sign-off before execution
    # (tool must already be in CapabilityToken — this gates execution, not grant)
    hilRequiredTools:
      - send_email
      - http_request
      - bash

    # capability_upgrade: whether HIL is required when agent requests a new tool
    # true = always HIL for any tool upgrade
    # false = auto-grant if user ACL permits (use with caution)
    capUpgradeRequiresHil: true

    blockedToolChains:
      - pattern: "email + code_exec"
        reason: high risk of data exfiltration

  models:
    default: claude-sonnet-4
    kernel: claude-opus-4          # requires restart
    fallback: claude-haiku-4
    temperature: 0.7

  observability:
    logLevel: info
    logPath: /var/log/avix/kernel.log
    metricsEnabled: true
    traceEnabled: false

  secrets:
    algorithm: aes-256-gcm
    masterKey:
      source: env                  # env | key-file | kms-aws | kms-gcp | kms-azure | kms-vault
      envVar: AVIX_MASTER_KEY      # for source: env
      # keyFile: ~/.config/avix/master.key   # for source: key-file
      # kmsKeyId: arn:aws:kms:...            # for source: kms-aws
    store:
      path: /secrets
      provider: local
    audit:
      enabled: true
      logPath: /var/log/avix/secrets-audit.log
      logReads: true
      logWrites: true
```

**IPC transport note:** `local-ipc` replaces the old `unix-socket | grpc (future)`. The kernel selects the correct OS mechanism automatically. Services and agents never need to know which mechanism is in use — they receive a resolved socket path via `AVIX_KERNEL_SOCK` env var.

-----

## 10. Users — /etc/avix/users.yaml

```yaml
apiVersion: avix/v1
kind: Users
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  users:
    - username: alice
      uid: 1001
      workspace: /users/alice/workspace
      shell: /bin/sh
      crews: [researchers, writers]
      additionalTools: [python]
      deniedTools: []
      quota:
        tokens: 500000
        agents: 5
        sessions: 4
```

Reserved UIDs: `0` = root, `1–99` = kernel internals, `100–999` = system agents.

-----

## 11. Crews — /etc/avix/crews.yaml

```yaml
apiVersion: avix/v1
kind: Crews
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  crews:
    - name: researchers
      cid: 1001
      members: [user:alice, agent:researcher]
      agentInheritance: spawn
      allowedTools: [web_search, web_fetch, file_read, file_write]
      deniedTools: [bash, send_email]
      sharedPaths: [/crews/researchers/shared/]
      pipePolicy: allow-intra-crew
```

-----

## 12. Services

All services live under `AVIX_ROOT/services/`. Built-in services are compiled into the `avix` binary. Installed services are added via ATP `sys.install` or `avix service install`. **At runtime the kernel treats them identically.**

### Services Are Language-Agnostic Host Processes

A service is any OS process that:

1. Reads `AVIX_SVC_TOKEN`, `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK` from env
1. Connects to `AVIX_KERNEL_SOCK` and sends `ipc.register`
1. Listens on `AVIX_SVC_SOCK` for incoming tool calls
1. Speaks JSON-RPC 2.0 with 4-byte length-prefix framing

The service binary can be written in Rust, Python, Go, Node, Ruby, or anything else. The wire protocol is the SDK.

### Built-in Services

|Service            |Description                                       |Key caps                              |
|-------------------|--------------------------------------------------|--------------------------------------|
|`router.svc`       |IPC backbone. Must start first.                   |—                                     |
|`auth.svc`         |Capability token authority.                       |`auth:admin`                          |
|`memfs.svc`        |VFS abstraction. Driver-swappable.                |`fs:read`, `fs:write`                 |
|`logger.svc`       |Structured log sink.                              |`fs:write`                            |
|`watcher.svc`      |File event bus.                                   |`fs:read`, `fs:watch`                 |
|`scheduler.svc`    |Crontab + timers.                                 |`fs:read`                             |
|`tool-registry.svc`|Scans `/tools/**/*.tool.yaml`.                    |`fs:read`                             |
|`jobs.svc`         |Long-running job broker.                          |`fs:read`, `fs:write`                 |
|`exec.svc`         |Code execution + runtime discovery.               |`exec:python`, `exec:js`, `exec:shell`|
|`mcp-bridge.svc`   |MCP protocol adapter.                             |`fs:read`                             |
|`gateway.svc`      |ATP WebSocket server. `:7700` user, `:7701` admin.|`auth:session`                        |
|`gui.svc`          |Browser UI server.                                |`fs:read`, `auth:session`             |
|`shell.svc`        |TTY interface.                                    |`fs:read`, `fs:write`, `auth:session` |

### Service Installation via ATP

```json
// ATP command — operator+ role required
{
  "type": "cmd", "domain": "sys", "op": "install",
  "body": {
    "type": "service",
    "source": "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
    "checksum": "sha256:abc123..."
  }
}
```

Install flow:

1. Download and verify checksum
1. Verify package signature
1. Conflict check (name, tool paths, ports)
1. Extract to `AVIX_ROOT/services/<name>/`
1. Write `service.unit`
1. Write `.install.json` receipt
1. Spawn process with env vars
1. `kernel/ipc/register`
1. `tool-registry.svc` rescan
1. Return `{ pid, tools[], status }`

### Service Identity

Services run as first-class kernel-managed processes with a service identity token (`AVIX_SVC_TOKEN`) analogous to an agent’s `CapabilityToken`. This token:

- Identifies the service in all IPC calls
- Scopes VFS writes to `/services/<name>/`
- Is issued at service start, held in memory, never written to disk

### Shared Services and Multi-User Security

When multiple users call the same service, the router injects `_caller` into every tool call:

```json
{
  "jsonrpc": "2.0",
  "method": "github/list-prs",
  "params": {
    "repo": "org/myrepo",
    "_caller": {
      "pid": 57,
      "user": "alice",
      "token": "eyJ..."
    }
  }
}
```

Services that serve multiple users declare `caller_scoped: true` in `service.unit` and use `_caller.user` to scope per-user behavior (e.g., resolving the correct credential from `/secrets/alice/`). The kernel enforces tool ACLs before the call reaches the service — unauthorized calls never arrive.

### Dynamic Tool Add/Remove

A service can add or remove tools at runtime based on external factors (API availability, auth state, feature flags):

```json
// Add tools
{ "jsonrpc": "2.0", "method": "ipc.tool-add",
  "params": { "_token": "<svc_token>", "tools": [{ "name": "github/list-prs", "descriptor": {...}, "visibility": "all" }] } }

// Remove tools
{ "jsonrpc": "2.0", "method": "ipc.tool-remove",
  "params": { "_token": "<svc_token>", "tools": ["github/list-prs"], "reason": "API unreachable", "drain": true } }
```

`drain: true` waits for in-flight calls to complete before removing. The kernel pushes a `tool.changed` ATP event to all subscribed clients.

**Tool states:** `available` | `degraded` | `unavailable`

-----

## 13. Agents

All agents live in `/bin/` (system) or `/users/<username>/bin/` (user-installed).

### Built-in Agents

|Agent           |Description                            |LLM required|Key caps                                        |
|----------------|---------------------------------------|:----------:|------------------------------------------------|
|`kernel.agent`  |System supervisor. Holds `kernel:root`.|Optional    |`kernel:root`, `llm:inference`                  |
|`planner.agent` |Task decomposition.                    |Yes         |`fs:read`, `llm:inference`                      |
|`executor.agent`|Tool execution loop.                   |Yes         |`fs:read`, `fs:write`, `exec:*`, `llm:inference`|
|`memory.agent`  |File indexing and context retrieval.   |Yes         |`fs:read`, `llm:inference`                      |
|`observer.agent`|System health monitoring.              |Optional    |`fs:read`, `kernel:root`                        |

-----

## 14. IPC Protocol

The IPC protocol is the internal communication layer between all Avix processes. It is completely separate from ATP.

### Transport

**Platform-native local sockets:**

|Platform        |Mechanism            |Socket path pattern    |
|----------------|---------------------|-----------------------|
|Linux           |AF_UNIX domain socket|`/run/avix/<name>.sock`|
|macOS           |AF_UNIX domain socket|`/run/avix/<name>.sock`|
|Windows ≥10 1803|Named Pipe           |`\\.\pipe\avix-<name>` |

The kernel resolves the platform path from a logical name. Config, env vars, and service code use only the logical name (`kernel`, `router`, etc.). The `AVIX_KERNEL_SOCK` and `AVIX_ROUTER_SOCK` env vars contain the already-resolved OS path.

### Wire Format

Every message is framed identically on all platforms:

```
┌─────────────────────────────────────────┐
│  4 bytes: payload length (uint32, LE)   │
├─────────────────────────────────────────┤
│  N bytes: UTF-8 JSON (JSON-RPC 2.0)     │
└─────────────────────────────────────────┘
```

Read 4 bytes → parse length → read N bytes → parse JSON. Implementable in any language with no dependencies beyond stdlib.

### Connection Model

The router opens a **fresh connection per tool call** — not a persistent multiplexed connection. This gives services natural per-call concurrency: each incoming connection is an independent call handled in its own thread/goroutine/async task.

### Service Startup Sequence

Every service — in any language — follows this sequence:

**1. Read environment:**

```
AVIX_KERNEL_SOCK  → resolved path to kernel socket
AVIX_ROUTER_SOCK  → resolved path to router socket
AVIX_SVC_SOCK     → resolved path for THIS service to listen on
AVIX_SVC_TOKEN    → service identity token
```

**2. Register with kernel:**

```json
{
  "jsonrpc": "2.0", "id": "1", "method": "ipc.register",
  "params": {
    "token": "<AVIX_SVC_TOKEN>",
    "name": "my-svc",
    "endpoint": "<AVIX_SVC_SOCK>",
    "tools": ["my-svc/tool-a", "my-svc/tool-b"]
  }
}
```

**3. Listen for incoming tool calls:**

```json
// Incoming (from router):
{
  "jsonrpc": "2.0", "id": "call-abc", "method": "my-svc/tool-a",
  "params": { "arg": "value", "_caller": { "pid": 57, "user": "alice", "token": "..." } }
}

// Response:
{ "jsonrpc": "2.0", "id": "call-abc", "result": { ... } }

// Error:
{ "jsonrpc": "2.0", "id": "call-abc", "error": { "code": -32001, "message": "..." } }
```

**4. Make outgoing calls (use router for tool calls, kernel for syscalls):**

```json
{ "jsonrpc": "2.0", "id": "out-1", "method": "fs/read",
  "params": { "path": "/services/my-svc/workspace/data.json", "_token": "<AVIX_SVC_TOKEN>" } }
```

**5. Handle inbound signals (JSON-RPC notifications, no response expected):**

```json
{ "jsonrpc": "2.0", "method": "signal", "params": { "signal": "SIGHUP", "payload": {} } }
{ "jsonrpc": "2.0", "method": "signal", "params": { "signal": "SIGTERM", "payload": {} } }
```

`SIGTERM` → finish in-flight calls → exit cleanly.

### Concurrency

Services handle concurrency by accepting multiple connections and processing each independently. The `service.unit` declares capacity limits:

```yaml
[service]
max_concurrent: 20    # router queues calls beyond this
queue_max:      100   # calls beyond this get EBUSY immediately
queue_timeout:  5s    # queued call timeout before ETIMEOUT
```

### Error Codes

|Code  |Meaning                           |
|------|----------------------------------|
|-32700|Parse error (JSON-RPC standard)   |
|-32601|Method not found                  |
|-32602|Invalid params                    |
|-32001|Auth failed — bad or expired token|
|-32002|Permission denied                 |
|-32003|Resource not found                |
|-32004|Rate limited / quota exceeded     |
|-32005|Tool unavailable                  |
|-32006|Conflict                          |
|-32007|Timeout                           |
|-32008|Service at capacity (EBUSY)       |

-----

## 15. Kernel Syscalls — /tools/kernel/

All 32 syscalls require `kernel:root` capability. All are synchronous.

|Domain              |Path                   |Count|Linux analog                        |
|--------------------|-----------------------|-----|------------------------------------|
|Process lifecycle   |`/tools/kernel/proc/`  |8    |`fork`, `exec`, `waitpid`, `kill`   |
|Signal bus          |`/tools/kernel/signal/`|4    |`kill`, `sigaction`                 |
|IPC registry        |`/tools/kernel/ipc/`   |7    |`bind`, `connect`, service discovery|
|Capability authority|`/tools/kernel/cap/`   |5    |`capset`, `/etc/sudoers`            |
|MemFS namespace     |`/tools/kernel/mem/`   |5    |`mount`, `umount`                   |
|System lifecycle    |`/tools/kernel/sys/`   |5    |`reboot`, `syslog`                  |

### kernel/proc/ — 8 syscalls

|Syscall |Key inputs                                    |Key outputs                               |Destructive|
|--------|----------------------------------------------|------------------------------------------|:---------:|
|`spawn` |`agent`, `parent_pid`, `capabilities`, `scope`|`pid`, `token`, `ipc_endpoint`            |No         |
|`kill`  |`pid`, `reason`                               |`killed`, `tokens_revoked`                |**Yes**    |
|`pause` |`pid`, `reason`, `prompt`                     |`paused`, `paused_at_tool`                |No         |
|`resume`|`pid`, `decision`                             |`resumed`, `status`                       |No         |
|`list`  |`filter_status`, `filter_type`                |`processes[]`                             |No         |
|`stat`  |`pid`                                         |full process detail                       |No         |
|`wait`  |`pid`, `timeout`                              |`final_status`, `exit_code`, `duration_ms`|No         |
|`setcap`|`pid`, `grant[]`, `revoke[]`                  |`capabilities[]`                          |No         |

### kernel/ipc/ — 7 syscalls (extended from v2)

|Syscall      |Description                            |
|-------------|---------------------------------------|
|`register`   |Register a service with its tools      |
|`deregister` |Remove a service registration          |
|`lookup`     |Find a service endpoint by name        |
|`list`       |List registered services               |
|`health`     |Check service health                   |
|`tool-add`   |Dynamically add tools to a service     |
|`tool-remove`|Dynamically remove tools from a service|

-----

## 16. Tool Namespace — /tools/

`/tools/` contains only `.tool.yaml` descriptor files. No executable code lives here. The in-memory registry (held by `tool-registry.svc`) reflects the currently available state — dynamic mutations do not write to disk.

### Tool Descriptor Format

```yaml
name:        read
path:        /tools/fs/read
owner:       memfs.svc
description: Read file contents from the active storage backend.
status:
  state: available           # available | degraded | unavailable
  reason: null
  retry_after: null
ipc:
  transport: local-ipc
  endpoint:  memfs            # logical name — kernel resolves to OS socket path
  method:    fs.read
streaming:   false
job:         false
capabilities_required: [fs:read]
input:
  path: { type: string, required: true }
output:
  content: { type: string }
```

-----

## 17. Storage Backends and Mount System

`memfs.svc` abstracts all storage. Same tool surface regardless of backend.

### Default Layout

All paths default to `AVIX_ROOT/<tree>`. Override specific subtrees in `fstab.yaml`.

### Mount Configuration — /etc/avix/fstab.yaml

Only declare overrides — unspecified paths fall back to `AVIX_ROOT` defaults:

```yaml
apiVersion: avix/v1
kind: Fstab

spec:
  mounts:
    # Secrets on a separate encrypted volume
    - path: /secrets
      provider: local
      config:
        root: /mnt/encrypted-vol/avix-secrets
      options:
        encrypted: true

    # User snapshots in cold S3
    - path: /users/alice/snapshots
      provider: s3
      config:
        bucket: avix-snapshots-prod
        prefix: alice/
        region: us-east-1
        auth: iam-role
      options:
        encrypted: true
```

### Storage Providers

|Provider            |Type string |Best for           |
|--------------------|------------|-------------------|
|Local disk          |`local`     |Dev, single-node   |
|S3-compatible       |`s3`        |Cloud, cold storage|
|Google Cloud Storage|`gcs`       |GCP                |
|Azure Blob          |`azure-blob`|Azure              |
|NFS                 |`nfs`       |On-prem shared     |
|In-memory           |`memory`    |Testing            |

-----

## 18. CapabilityToken

Issued by the kernel on agent spawn. Passed as `AVIX_CAP_TOKEN` env var. Agents present
this token on every ResourceRequest. The kernel validates signature + expiry before acting.

```yaml
apiVersion: avix/v1
kind: CapabilityToken
metadata:
  issuedAt: 2026-03-15T07:38:00-05:00
  expiresAt: 2026-03-15T08:38:00-05:00
  issuedTo:
    pid: 57
    agentName: researcher
    spawnedBy: alice
spec:
  tools:
    granted: [web_search, web_fetch, file_read]
    # send_email absent — requires HIL capability_upgrade to add
  constraints:
    maxTokensPerTurn: 8000
    maxToolChainLength: 8
    allowPipeTargets: [58]
    tool_call_budget: {}        # per-tool call limits; populated on scope:once upgrades
                                # e.g. { send_email: 1 } — kernel enforces per dispatch
  signature: sha256:tokenSig789...  # HMAC-signed; any modification invalidates
```

### Token Lifecycle

```
spawn → CapabilityToken issued (tools from crew + user ACL intersection)
  │
  ├── normal operation: agent presents token on every ResourceRequest
  │
  ├── token_renewal: agent sends ResourceRequest { resource: token_renewal }
  │     kernel issues fresh token, same grants, new expiry + signature
  │
  ├── capability_upgrade HIL approved (scope: session):
  │     kernel issues NEW token with tool added to spec.tools.granted
  │     delivered to agent via SIGRESUME payload: { new_capability_token: "..." }
  │     agent RuntimeExecutor replaces its held token
  │
  ├── capability_upgrade HIL approved (scope: once):
  │     kernel issues NEW token with tool added AND tool_call_budget: { tool: 1 }
  │     kernel auto-revokes grant when budget hits 0
  │
  └── agent exit / SIGKILL: token invalidated in kernel's active token table
```

**Key rules:**

- `spec.tools.granted` lists only tools actually granted — absent tools require HIL `capability_upgrade`
- Tokens are HMAC-signed — any modification invalidates them
- Agents treat tokens as opaque strings — never parse internal structure
- New tokens from capability upgrades carry the same `expiresAt` as the replaced token
- `tool_call_budget` is enforced by the kernel at tool dispatch, not by the agent

Services receive an analogous `ServiceToken` (`AVIX_SVC_TOKEN`) at startup, scoped to `/services/<n>/`.

-----

## 19. ResourceRequest and ResourceResponse

Agents send ResourceRequests to `AVIX_KERNEL_SOCK` when they need resources not granted
at spawn time, or when a tool call requires human approval before execution.

```yaml
apiVersion: avix/v1
kind: ResourceRequest
metadata:
  agentPid: 57
  requestId: req-abc123
  capabilityToken: sha256:tokenSig789...
spec:
  requests:
    # Tool NOT in CapabilityToken → triggers HIL capability_upgrade if policy requires it
    - resource: tool
      name: send_email
      reason: "Need to notify user when analysis complete"
      urgency: low              # low | normal | high — informs HIL queue priority

    # Tool IS in token but policy requires approval for this specific call
    - resource: tool_call_approval
      tool: send_email
      args: { to: "team@org.com", subject: "Summary ready" }
      reason: "Sending research summary"
      urgency: normal

    # Standard token renewal (always auto-approved)
    - resource: token_renewal
      reason: Token expires in 5 minutes

    # Context token expansion
    - resource: context_tokens
      amount: 50000
      reason: Need longer research thread
```

### ResourceResponse — HIL-pending vs granted

```yaml
apiVersion: avix/v1
kind: ResourceResponse
metadata:
  requestId: req-abc123
  respondedAt: 2026-03-15T07:38:25-05:00
spec:
  grants:
    # Tool request → HIL pending (kernel sent SIGPAUSE, opened hil.request ATP event)
    - resource: tool
      name: send_email
      granted: false
      hil_pending: true         # agent must wait for SIGRESUME before proceeding
      hilId: hil-002
      reason: "Capability upgrade requires human approval"

    # Tool call approval → HIL pending
    - resource: tool_call_approval
      tool: send_email
      granted: false
      hil_pending: true
      hilId: hil-001
      reason: "Tool call requires human approval"

    # Token renewal → always auto-approved
    - resource: token_renewal
      granted: true
      expiresAt: 2026-03-15T09:38:00-05:00

    # Context tokens → auto-approved if within quota
    - resource: context_tokens
      granted: true
      amount: 50000
      newTotal: 114000
```

`hil_pending: true` means: the kernel has opened a HIL event, sent `SIGPAUSE` to the
agent, and pushed a `hil.request` ATP event to connected clients. The agent is suspended
and must not proceed until it receives `SIGRESUME`. The `hilId` correlates the eventual
`SIGRESUME` payload back to this specific request.

-----

## 20. Signals

Signals are delivered as JSON-RPC notifications on the agent’s per-PID socket
(`/run/avix/agents/<pid>.sock`). No response is sent or expected.

```json
{ "jsonrpc": "2.0", "method": "signal",
  "params": { "signal": "SIGPAUSE",
              "payload": { "hilId": "hil-001", "type": "tool_call_approval",
                           "reason": "send_email requires human approval" } } }
```

|Signal       |Direction     |Meaning                                                              |
|-------------|--------------|---------------------------------------------------------------------|
|`SIGSTART`   |Kernel → Agent|Begin execution                                                      |
|`SIGPAUSE`   |Kernel → Agent|Suspend at next tool boundary; payload carries `hilId` for HIL pauses|
|`SIGRESUME`  |Kernel → Agent|Resume; payload carries HIL decision when resuming from a HIL pause  |
|`SIGKILL`    |Kernel → Agent|Terminate immediately                                                |
|`SIGSTOP`    |Kernel → Agent|Stop (session closed)                                                |
|`SIGSAVE`    |Kernel → Agent|Take a snapshot now                                                  |
|`SIGPIPE`    |Kernel → Agent|Pipe established/closed                                              |
|`SIGESCALATE`|Agent → Kernel|Agent proactively requests human escalation; agent pauses itself     |

### SIGRESUME payload variants

**tool_call_approval approved:**

```json
{ "hilId": "hil-001", "decision": "approved",
  "note": "Looks good, send it" }
```

**tool_call_approval denied:**

```json
{ "hilId": "hil-001", "decision": "denied",
  "reason": "Don't send to that address" }
```

**capability_upgrade approved:**

```json
{ "hilId": "hil-002", "decision": "approved", "scope": "session",
  "new_capability_token": "<full new HMAC-signed token>" }
```

**escalation responded:**

```json
{ "hilId": "hil-003", "decision": "approved",
  "selectedOption": "exclude",
  "guidance": "Exclude salary data. Focus on revenue and margins only." }
```

**Any type — timeout:**

```json
{ "hilId": "hil-001", "decision": "timeout",
  "reason": "No response within hil_timeout" }
```

Agent RuntimeExecutor treats `timeout` identically to `denied`.

### SIGESCALATE payload (Agent → Kernel)

```json
{ "signal": "SIGESCALATE",
  "payload": {
    "reason": "Found PII in dataset. Unsure whether to include in report.",
    "context": "Researching Q3 financials, found employee salary data...",
    "options": [
      { "id": "include", "label": "Include with redaction" },
      { "id": "exclude", "label": "Exclude entirely" }
    ]
  }
}
```

The kernel mints an `ApprovalToken`, writes a HIL record, and pushes a `hil.request`
event of type `escalation`. The agent is already paused (it sent the signal and waits).

-----

## 21–24. Pipes, Snapshots, Crontab, Defaults and Limits

*Unchanged from v2. See respective spec files.*

-----

## 25. Resolved Config

*Unchanged from v2.*

-----

## 26. Session and Capability Model

### Core Principle: Policy persists. Tokens do not.

`/etc/avix/auth.conf` survives restarts. Active tokens do not. Tokens are always freshly derived from policy at session start.

### Credential Types

**Removed from v3:**

- `credential.type: none` — removed. All callers use `api_key` or `password`.
- `auto_session: true` — removed. Desktop app uses OS keychain instead.

**Active in v3:**

- `password` — argon2id hash, for interactive multi-user setups
- `api_key` — HMAC-SHA256 hash, for all automated callers

### Role Hierarchy

```
admin    ← full system control
  operator  ← spawn/kill agents, manage services, view all logs
    user     ← spawn own agents, manage own workspace
      guest  ← read-only
```

-----

## 27. Avix Terminal Protocol (ATP)

ATP is the sole external interface. All client-facing operations — spawning agents, managing users, installing services, watching events — are ATP commands.

See [ATP_Spec.md](./ATP_Spec.md) for the full protocol specification.

### Quick Reference

**Endpoint:** `wss://localhost:7700/atp` (user), `wss://localhost:7701/atp` (admin)

**Auth flow:**

```
POST /atp/auth/login { identity, credential }
→ { token, expiresAt, sessionId }

GET /atp + Authorization: Bearer <token>
→ 101 Switching Protocols
→ { type: "event", event: "session.ready" }
```

**Every command frame:**

```json
{ "type": "cmd", "id": "c-001", "token": "<ATPToken>",
  "domain": "proc", "op": "spawn", "body": { ... } }
```

**Token re-validated on every message** — revocation is immediate.

**Command domains:** `auth`, `proc`, `signal`, `fs`, `snap`, `cron`, `users`, `crews`, `cap`, `sys`, `pipe`

**Key ATP events:** `session.ready`, `token.expiring`, `agent.output`, `agent.status`, `hil.request`, `tool.changed`, `fs.changed`, `sys.alert`

**Admin port rule:** `sys` and `cap` domains only accepted on port `7701` — even an admin token on `7700` cannot invoke system lifecycle operations.

-----

## 28. exec.svc — Runtime Discovery

Discovers host runtimes at boot + on SIGHUP.

Discovery pipeline: shell env → runtimes → tools → package managers → write manifest → notify tool-registry.

```
/tools/exec/
├── runtime/python/run | repl
├── runtime/node/run | repl
├── runtime/shell/run | repl
├── tool/git/... | docker/... | curl/... | jq/...
└── pkg/uv/... | npm/... | pip/...
```

-----

## 29. mcp-bridge.svc

Connects to MCP servers (static from `mcp.d/*.mcp.yaml` or dynamic via ATP). Generates `.tool.yaml` descriptors under `/tools/mcp/<server>/`. Agents call MCP tools identically to built-in tools.

-----

## 30. jobs.svc — Long-Running Jobs

Any tool whose descriptor declares `job: true` returns a `job_id` immediately. Work runs asynchronously. Results flow via events.

### Job Call Contract

**Synchronous tool:** call → wait → result (connection stays open, milliseconds)

**Job tool:** call → `{ job_id }` → connection closes → background work → events via `jobs.svc`

### Service Emitting Job Events

```json
// Progress
{ "jsonrpc": "2.0", "method": "jobs.emit",
  "params": { "_token": "<svc_token>", "job_id": "job-7f3a", "event": { "type": "progress", "percent": 45 } } }

// Completion
{ "jsonrpc": "2.0", "method": "jobs.complete",
  "params": { "_token": "<svc_token>", "job_id": "job-7f3a", "result": { ... } } }

// Failure
{ "jsonrpc": "2.0", "method": "jobs.fail",
  "params": { "_token": "<svc_token>", "job_id": "job-7f3a", "error": { "code": -32001, "message": "..." } } }
```

### Agent Following a Job

```json
{ "jsonrpc": "2.0", "id": "sub-1", "method": "jobs/watch",
  "params": { "_token": "<cap_token>", "job_id": "job-7f3a" } }
```

Connection stays open. `jobs.svc` streams events as they arrive.

### service.unit Job Controls

```yaml
[service]
max_concurrent: 5      # simultaneous IPC connections
queue_max:      20
queue_timeout:  10s

[jobs]
max_active:  3         # simultaneous background workers
job_timeout: 3600s     # kernel marks failed after this
persist:     false     # true = job survives service restart
```

### Job States

`pending → running → done`  
`running → paused → running`  
`running → failed`

-----

## 31. Secrets Store

Secrets are AES-256-GCM encrypted blobs. The master key lives only in kernel memory — never written to any file.

```
At write time:  plaintext → AES-256-GCM(master_key, nonce) → /secrets/<user>/<name>.enc
At read time:   kernel reads .enc → decrypt → inject into agent context only
                never exposed via VFS — ever
```

### Master Key Sources

|Source     |Use case                                                         |
|-----------|-----------------------------------------------------------------|
|`env`      |Container/app (key injected at launch, zeroed after Phase 2)     |
|`key-file` |CLI personal use (file at `~/.config/avix/master.key`, chmod 600)|
|`kms-aws`  |AWS — IAM role, no key material on disk                          |
|`kms-gcp`  |GCP Cloud KMS                                                    |
|`kms-azure`|Azure Key Vault                                                  |
|`kms-vault`|HashiCorp Vault Transit                                          |

**Desktop app master key:** derived fresh at launch via `HKDF(machine_id + app_bundle_id)`. Machine-bound — useless on another machine even with copied files.

-----

## 32. Installation and Packaging

### avix config init

Generates `/etc/avix/` config files. Must run before `avix start`. See §8.

### avix install

Extracts built-in assets into `AVIX_ROOT`. Run once on first boot, again after binary upgrade.

```
01  Read AVIX_ROOT from env or --root flag
02  Verify /etc/avix/auth.conf exists — abort if not (run avix config init first)
03  Create directory tree with correct permissions
04  Extract built-in service assets → AVIX_ROOT/services/<n>/
05  Extract built-in agent files → AVIX_ROOT/bin/<n>/
06  Write AVIX_ROOT/services/.manifest.json (SHA-256 checksums)
07  Done → avix start
```

### avix start (boot verification)

```
read AVIX_ROOT/services/.manifest.json
compare against manifest embedded in binary
mismatch → abort: "ring-1 tampered. Run: avix install"
         → unless: --allow-modified-ring-1 (dev mode only)
verify /etc/avix/auth.conf exists and is valid YAML
match + valid → proceed with bootstrap
```

### avix service install / avix agent install

CLI equivalents to ATP `sys.install`. Both write a `.install.json` receipt:

```json
{
  "name": "github-svc",
  "version": "1.2.0",
  "source": "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
  "checksum": "sha256:abc123...",
  "signature": "sha256:def456...",
  "installedAt": "2026-03-20T10:00:00Z",
  "installedBy": "alice",
  "autostart": true
}
```

-----

## 33. Unit File Format

### service.unit

```toml
name        = github-svc
version     = 1.2.0
source      = community          # system | community | user
signature   = sha256:abc...

[unit]
description   = GitHub integration service
requires      = [router, auth, memfs]
after         = [auth]

[service]
binary        = /services/github-svc/bin/github-svc
language      = go               # informational only — kernel doesn't care
restart       = on-failure
restart_delay = 5s
run_as        = service          # service | user:<username>
                                 # service = scoped to /services/<name>/
                                 # user:<name> = single-user installs only
max_concurrent = 20
queue_max      = 100
queue_timeout  = 5s

[capabilities]
required     = [fs:read, fs:write]
scope        = /services/github-svc/
host_access  = [network]         # network | filesystem:<path> | socket:<path> | env:<VAR>
caller_scoped = true             # kernel injects _caller on every tool call

[tools]
namespace    = /tools/github/
provides     = [list-prs, create-issue, search-code, get-file]

[jobs]
max_active   = 3
job_timeout  = 3600s
persist      = false
```

### agent.unit

```toml
name         = executor.agent
binary       = avix --agent=executor.agent

[unit]
description  = Tool execution loop
requires     = [kernel.agent, tool-registry, exec]
spawned_by   = kernel.agent

[agent]
type         = worker            # supervisor | worker | monitor
restart      = on-failure

[capabilities]
required     = [fs:read, fs:write, exec:python, exec:js, exec:shell, llm:inference]
scope        = /home/

[tools]
allowed      = [/tools/fs/**, /tools/exec/**, /tools/mcp/**, /tools/jobs/**]
denied       = [/tools/kernel/**]

[jobs]
max_concurrent = 4
```

-----

## 34. Validation Rules

1. Every file must have `apiVersion: avix/v1` and a valid `kind`
1. `metadata.name` must match `^[a-z0-9][a-z0-9\-]{1,62}$`
1. `spec` required on all authored files
1. Unknown top-level fields cause a validation error
1. All timestamps must be ISO 8601 with timezone
1. `CapabilityToken.spec.signature` must be verified before any `ResourceRequest`
1. `Snapshot.checksum` must be verified before restore
1. Crontab schedules validated as standard 5-field cron (UTC)
1. UIDs below 1000 are reserved
1. `AgentManifest.spec.tools.required` must be a subset of the spawning user’s effective tool set
1. `auth.conf` must exist before `avix start` — no fallback boot mode
1. `credential.type: none` is invalid — rejected by validator
1. `service.unit caller_scoped: true` requires `_caller` handling in service code (advisory — enforced by audit)
1. `host_access` declarations validated at install time against operator-approved capabilities

-----

## 35. Open Questions

**exec.svc:**

- `pyenv`/`nvm`/`asdf` version switching mid-session
- Back-pressure policy when `max_concurrent` is saturated system-wide

**jobs.svc:**

- Opt-in job persistence (`persist: true`) — storage schema and recovery flow
- Retention policy for completed job directories

**Signals:**

- `SIGPAUSE` interaction when an agent is mid-IPC-call
- Signal propagation across piped agent chains

**Agents:**

- System prompt design for `kernel.agent`
- Memory agent indexing strategy — full-text, vector, or both
- Community agent vetting / signature verification model

**MCP:**

- Error handling when MCP server goes offline mid-session
- Tool descriptor caching — regenerate always vs. persist across restarts

**Crews:**

- Crew-level token budget design (v2)
- Capability resolution when a user belongs to many crews with conflicting tool grants

**Secrets:**

- Rotation flows for secrets used by long-running agents
- Portability when two instances share a KMS key

**Performance:**

- MemFS driver performance under S3 latency
- Process table size limits before scheduler.svc needs throttling
- IPC socket backpressure under high concurrent service load

