# 07 — Services

> Service lifecycle, installation, tool registration, caller injection, restart watchdog,
> and service secrets. Reflects implementation as of svc-gaps A–H.

---

## Overview

Services are the deterministic backbone of Avix. They are always-available OS processes
that expose tools via IPC. A service requires no LLM — if a deterministic program can
solve it, it is a service.

Services are **language-agnostic host processes**. Any language that can open a socket and
speak JSON-RPC 2.0 with 4-byte length-prefix framing can implement a service.

All services live under `AVIX_ROOT/services/`. Built-in services are compiled into the
`avix` binary. Installed services are added via `avix service install` or the ATP
`sys/install` syscall. **At runtime the kernel treats built-in and installed services
identically.**

---

## Service Contract

A service is any OS process that:

1. Reads `AVIX_SVC_TOKEN`, `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK` from env
2. Connects to `AVIX_KERNEL_SOCK` and sends `ipc.register`
3. Listens on `AVIX_SVC_SOCK` for incoming tool calls
4. Speaks JSON-RPC 2.0 with 4-byte little-endian length-prefix framing

See `03-ipc.md` for the full wire protocol.

---

## Service Lifecycle

```
avix start
    │
    ├─ ServiceManager::spawn_and_get_token()  → issues ServiceToken (PID + token str)
    │
    ├─ ServiceProcess::spawn()               → forks binary, injects env vars
    │       AVIX_SVC_TOKEN    = svc-token-<uuid>
    │       AVIX_KERNEL_SOCK  = /run/avix/kernel.sock
    │       AVIX_ROUTER_SOCK  = /run/avix/router.sock
    │       AVIX_SVC_SOCK     = /run/avix/services/<name>-<pid>.sock
    │
    ├─ Service starts, binds AVIX_SVC_SOCK
    │
    ├─ Service → ipc.register → kernel
    │       Kernel validates token, records endpoint, scans *.tool.yaml from
    │       AVIX_ROOT/services/<name>/tools/ and registers descriptors in ToolRegistry
    │
    ├─ Service optionally calls ipc.tool-add / ipc.tool-remove at runtime
    │
    └─ ServiceWatchdog polls every 5 s — restarts if policy = always | on-failure
```

---

## Built-in Services

| Service | Description | Key capabilities |
|---------|-------------|-----------------|
| `router.svc` | IPC backbone. Must start first. | — |
| `auth.svc` | Capability token authority. | `auth:admin` |
| `memfs.svc` | VFS abstraction. Driver-swappable. | `fs:read`, `fs:write` |
| `logger.svc` | Structured log sink. | `fs:write` |
| `watcher.svc` | File event bus. | `fs:read`, `fs:watch` |
| `scheduler.svc` | Crontab runner. Loads `/etc/avix/crontab.yaml`. | `fs:read`, `proc:spawn` |
| `memory.svc` | Agent memory: episodic + semantic + retrieval. | `fs:read`, `fs:write` |
| `jobs.svc` | Long-running job broker. | `fs:read`, `fs:write` |
| `exec.svc` | Code execution + runtime discovery. | `exec:python`, `exec:js`, `exec:shell` |
| `mcp-bridge.svc` | MCP protocol adapter. | `fs:read` |
| `gateway.svc` | ATP WebSocket server. | `auth:session` |
| `llm.svc` | All AI inference. Owns all provider calls. | `llm:inference` |

**`llm.svc` is special:** `RuntimeExecutor` never calls provider APIs directly. All AI
inference goes through `llm.svc` via IPC.

---

## service.yaml Format (YAML)

Every installed service ships a `service.yaml` file at the root of its package. The
parser lives at `crates/avix-core/src/service/yaml.rs`.

```yaml
name: github-svc
version: 1.2.0

unit:
  description: GitHub integration service
  author: example.com/github-svc
  after:
    - auth.svc
    - memfs.svc
  requires:
    - auth.svc

service:
  binary: /services/github-svc/bin/github-svc
  language: rust
  restart: on-failure   # always | on-failure | never
  restart_delay: 5s    # parsed by parse_duration()
  max_concurrent: 20  # dispatcher concurrency limit
  queue_max: 100
  queue_timeout: 5s
  run_as: service     # service | user | root

capabilities:
  caller_scoped: true            # inject _caller into every tool call
  host_access:                   # none | network | filesystem | all
    - network

tools:
  namespace: /tools/github/
  provides: []                  # explicit tool list (empty = scan tools/ dir)

jobs:
  enabled: false
```

### RestartPolicy values

| Value | Behaviour |
|-------|-----------|
| `always` | Restart on any exit |
| `on-failure` | Restart on any exit (simplified — all exits treated as failure) |
| `never` | Do not restart |

---

## Tool Descriptor Files (`*.tool.yaml`)

Services place typed tool descriptors in `AVIX_ROOT/services/<name>/tools/`. The
`ToolScanner` reads these at `ipc.register` time and populates the `ToolRegistry`.
Implementation: `crates/avix-core/src/tool_registry/scanner.rs`.

```yaml
name:        github/list-prs
description: List open pull requests for a repository.
status:
  state: available          # available | degraded | unavailable
  reason: null
ipc:
  transport: local-ipc
  endpoint:  github-svc
  method:    github.list-prs
streaming: false
job:       false
capabilities_required: [github:read]
visibility: all             # all | {user: alice} | {crew: engineering}
input:
  repo: { type: string, required: true }
output:
  prs: { type: array }
```

`ToolVisibilitySpec` controls which users see the tool in their tool list:
- `all` — visible to every user
- `{user: alice}` — visible only to user `alice`
- `{crew: engineering}` — visible only to members of crew `engineering`

---

## IPC Wire Protocol

### `ipc.register` (service → kernel at startup)

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "ipc.register",
  "params": {
    "_token":   "svc-token-<uuid>",
    "name":     "github-svc",
    "endpoint": "/run/avix/services/github-svc-42.sock",
    "tools":    []
  }
}
```

On success, the kernel:
1. Validates the token against the `ServiceManager` token map
2. Records the endpoint in `ServiceRegistry`
3. Scans `AVIX_ROOT/services/<name>/tools/*.tool.yaml`
4. Registers all discovered `ToolEntry` records in `ToolRegistry`
5. Stamps `registered_at` on the `ServiceRecord`

### `ipc.tool-add` (service → kernel, runtime update)

```json
{
  "jsonrpc": "2.0",
  "id": "2",
  "method": "ipc.tool-add",
  "params": {
    "_token": "svc-token-<uuid>",
    "tools": [
      {
        "name":       "github/list-prs",
        "descriptor": { "description": "List open PRs", "streaming": false },
        "visibility": "all"
      }
    ]
  }
}
```

Typed by `IpcToolAddParams` / `IpcToolSpec` in `service/lifecycle.rs`.

### `ipc.tool-remove` (service → kernel, runtime update)

```json
{
  "jsonrpc": "2.0",
  "id": "3",
  "method": "ipc.tool-remove",
  "params": {
    "_token": "svc-token-<uuid>",
    "tools":  ["github/list-prs"],
    "reason": "GitHub API unreachable",
    "drain":  true
  }
}
```

Typed by `IpcToolRemoveParams` in `service/lifecycle.rs`.

`drain: true` marks all named tools `Unavailable`, waits for all in-flight
`ToolCallGuard` permits to drain, then removes. `drain: false` removes immediately.

The kernel pushes a `tool.changed` ATP event to all subscribed clients when tools change.

---

## Multi-User Security — `_caller` Injection

When multiple users call the same service, the router injects a `_caller` object into
every tool call's params. Implementation: `crates/avix-core/src/router/caller.rs`,
`router/dispatcher.rs`, `router/registry.rs`.

### How it works

1. `service.yaml` declares `caller_scoped: true` under `capabilities`
2. At spawn, `ServiceManager` records `caller_scoped: true` on the `ServiceRecord`
3. When the router dispatches a call, it checks
   `ServiceRegistry::is_caller_scoped(svc_name)` — set via `register_with_meta()`
4. If scoped, `CallerInfo::inject_into(&mut request.params)` adds `_caller`

```json
{
  "jsonrpc": "2.0",
  "method": "github/list-prs",
  "params": {
    "repo": "org/myrepo",
    "_caller": {
      "pid":   57,
      "user":  "alice",
      "token": "eyJ..."
    }
  }
}
```

Services use `_caller.user` to scope per-user behaviour (e.g., resolve the correct
credential from `/secrets/user/<user>/`).

**The kernel enforces tool ACLs before the call reaches the service** — unauthorized
calls never arrive. Services can trust `_caller` as authoritative.

### Key types

```rust
// crates/avix-core/src/router/caller.rs
pub struct CallerInfo {
    pub pid:   u64,
    pub user:  String,
    pub token: String,
}
```

```rust
// crates/avix-core/src/router/registry.rs
impl ServiceRegistry {
    pub async fn register_with_meta(&self, name: &str, endpoint: &str, caller_scoped: bool);
    pub async fn is_caller_scoped(&self, name: &str) -> bool;
}
```

---

## Service Installation (`ServiceInstaller`)

Implementation: `crates/avix-core/src/service/installer.rs`.

### 7-Step Installation Pipeline

1. **Fetch** — `file://` path copy or `https://` URL download via `reqwest`
2. **Verify checksum** — SHA-256, format `"sha256:<hex>"`; error on mismatch
3. **Extract tarball** — strips top-level directory; sets `0o755` on `bin/` entries (Unix)
4. **Validate manifest** — errors if no `service.yaml` found in the tarball
5. **Conflict check** — errors if `AVIX_ROOT/services/<name>/` already exists
6. **Copy to install dir** — walks extracted tree with `walkdir`, copies all files
7. **Write receipt** — writes `.install.json` (`InstallReceipt`) with name, version,
   install timestamp, source URL, and tool list

### ATP Command

```json
{
  "type": "cmd", "domain": "sys", "op": "install",
  "body": {
    "source":    "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
    "checksum":  "sha256:abc123...",
    "autostart": true
  }
}
```

Requires `auth:admin` capability. Implemented in `syscall/domain/sys_.rs`.

---

## Service CLI Commands

```bash
# Install from local package or URL
avix service install <path-or-url> [--checksum sha256:…] [--no-start]

# List all installed services (offline — reads service.unit files from disk)
avix service list [--root ~/avix-data]

# Show service status from /proc/services/<name>/status.yaml
avix service status <name> [--root ~/avix-data]

# Lifecycle control (sends signals via ATP)
avix service start   <name>
avix service stop    <name>
avix service restart <name>

# Remove service from disk (--force kills first)
avix service uninstall <name> [--force] [--root ~/avix-data]

# Stream logs
avix service logs <name> [--follow]
```

---

## Restart Watchdog (`ServiceWatchdog`)

Implementation: `crates/avix-core/src/service/watchdog.rs`.

The watchdog is a background Tokio task that monitors registered service processes and
restarts them according to their `RestartPolicy`.

```rust
pub struct ServiceWatchdog {
    entries: Arc<RwLock<HashMap<String, WatchdogEntry>>>,
    _handle: JoinHandle<()>,
}

pub struct WatchdogEntry {
    pub unit:          ServiceUnit,
    pub process:       ServiceProcess,
    pub restart_count: u32,
}
```

### Restart Loop (5-second poll)

For each registered service:

1. Check `process.is_running()` — skip if still alive
2. Apply `RestartPolicy`:
   - `Always` / `OnFailure` → restart after `restart_delay`
   - `Never` → skip
3. Call `ServiceManager::respawn_token(name)` — issues a fresh `ServiceToken` with new PID
4. Call `ServiceProcess::spawn(&unit, &token, ...)` — re-forks the binary
5. Increment `entry.restart_count`

`restart_count` is available via `watchdog.restart_count("svc-name").await`.

---

## Service Secrets (`SecretStore` + `kernel/secret/get`)

### On-Disk Storage

Secrets are stored encrypted at:

```
AVIX_ROOT/secrets/<owner-type>/<owner-name>/<secret-name>.enc
```

Examples:
```
secrets/service/github-svc/app-key.enc
secrets/user/alice/gh-token.enc
```

Each `.enc` file contains **hex-encoded** AES-256-GCM ciphertext (12-byte nonce
prepended). The file is never valid plaintext.

Implementation: `crates/avix-core/src/secrets/store.rs` (`SecretStore` struct).

```rust
pub struct SecretStore {
    root:       PathBuf,
    master_key: [u8; 32],   // derived from AVIX_MASTER_KEY
}

impl SecretStore {
    pub fn new(root: &Path, key: &[u8]) -> Self;
    pub fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), SecretsError>;
    pub fn get(&self, owner: &str, name: &str)              -> Result<String, SecretsError>;
    pub fn delete(&self, owner: &str, name: &str)           -> Result<(), SecretsError>;
    pub fn list(&self, owner: &str)                         -> Vec<String>;
}
```

Owner format: `"service:<name>"` or `"user:<name>"`.

### `kernel/secret/get` Syscall

```json
{
  "method": "kernel/secret/get",
  "params": { "owner": "service:github-svc", "name": "app-key" }
}
```

Permission rules:
- Caller must have `kernel/secret/get` in granted tools
- `service:*` owners: caller's `issued_to.agent_name` must match, **or** caller has `auth:admin`
- `user:*` owners: any authorised caller may read

### Secrets CLI Commands

```bash
# All require AVIX_MASTER_KEY in env (direct filesystem operation, no ATP needed)
avix secret set <name> <value> --for-service <svc>  [--root ~/avix-data]
avix secret set <name> <value> --for-user    <user> [--root ~/avix-data]
avix secret list  --for-service <svc>
avix secret list  --for-user    <user>
avix secret delete <name> --for-service <svc>
```

---

## Service Status File

The kernel writes `/proc/services/<name>/status.yaml` at service spawn:

```yaml
name:          github-svc
version:       1.2.0
pid:           42
state:         Running    # Starting | Running | Degraded | Stopping | Stopped | Failed
endpoint:      /run/avix/services/github-svc-42.sock
tools:         [github/list-prs, github/create-pr]
restart_count: 0
```

---

## Dynamic Tool States

| State | Meaning |
|-------|---------|
| `Available` | Tool is healthy and callable |
| `Degraded` | Tool is operational but degraded (e.g., rate-limited) |
| `Unavailable` | Tool is not callable; dispatcher returns `EUNAVAIL (-32005)` |

`Unavailable` is set during `drain: true` removal while in-flight calls complete.

---

## RouterDispatcher — 8-Step Dispatch Flow

Implementation: `crates/avix-core/src/router/dispatcher.rs`.

| Step | Action | Error on failure |
|------|--------|-----------------|
| 1 | Look up tool in `ToolRegistry` | `ENOTFOUND (-32601)` |
| 2 | Reject if `ToolState::Unavailable` | `EUNAVAIL (-32005)` |
| 3 | Check caller capability | `EPERM (-32002)` |
| 4 | Acquire `ToolCallGuard` | `EUNAVAIL (-32005)` |
| 5 | Acquire global concurrency slot | `EBUSY (-32008)` |
| 6 | Resolve owning service → endpoint | `EUNAVAIL (-32005)` |
| 7 | Inject `_caller` if `caller_scoped` | — |
| 8 | Forward via `IpcClient::call()` | `ETIMEOUT (-32007)` |

---

## Filesystem Ownership for Services

| Path | Owner | Writable by service? |
|------|-------|:-------------------:|
| `AVIX_ROOT/services/<name>/` | Installer / kernel | No |
| `AVIX_ROOT/services/<name>/workspace/` | Service | Yes |
| `AVIX_ROOT/proc/services/<name>/status.yaml` | Kernel | No |
| `AVIX_ROOT/secrets/service/<name>/` | Admin CLI / kernel | No (kernel-injected) |

---

## Kernel Syscalls Relevant to Services

| Syscall | Description |
|---------|-------------|
| `sys/install` | Install a service package from URL or local path (`auth:admin` required) |
| `kernel/secret/get` | Retrieve an encrypted secret (service or user owner) |

IPC-layer methods (called by the service binary directly, not through the syscall table):

| Method | Description |
|--------|-------------|
| `ipc.register` | Register at startup — validates token, scans tool descriptors |
| `ipc.tool-add` | Add or update tools dynamically at runtime |
| `ipc.tool-remove` | Remove tools, optionally draining in-flight calls |
