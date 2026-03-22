# 07 — Services

> Service lifecycle, built-in services, identity, multi-user security, and dynamic tools.

---

## Overview

Services are the deterministic backbone of Avix. They are always-available OS processes that
expose tools via IPC. A service requires no LLM — if a deterministic program can solve it,
it is a service.

Services are **language-agnostic host processes**. Any language that can open a socket and
speak JSON-RPC 2.0 with 4-byte length-prefix framing can implement a service.

All services live under `AVIX_ROOT/services/`. Built-in services are compiled into the
`avix` binary. Installed services are added via ATP `sys.install` or `avix service install`.
**At runtime the kernel treats built-in and installed services identically.**

---

## Service Contract

A service is any OS process that:

1. Reads `AVIX_SVC_TOKEN`, `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK` from env
2. Connects to `AVIX_KERNEL_SOCK` and sends `ipc.register`
3. Listens on `AVIX_SVC_SOCK` for incoming tool calls
4. Speaks JSON-RPC 2.0 with 4-byte length-prefix framing

See `03-ipc.md` for the full wire protocol.

---

## Built-in Services

| Service | Description | Key capabilities |
|---------|-------------|-----------------|
| `router.svc` | IPC backbone. Must start first. | — |
| `auth.svc` | Capability token authority. | `auth:admin` |
| `memfs.svc` | VFS abstraction. Driver-swappable. | `fs:read`, `fs:write` |
| `logger.svc` | Structured log sink. | `fs:write` |
| `watcher.svc` | File event bus. | `fs:read`, `fs:watch` |
| `scheduler.svc` | Crontab + timers. | `fs:read` |
| `tool-registry.svc` | Scans `/tools/**/*.tool.yaml`. | `fs:read` |
| `jobs.svc` | Long-running job broker. | `fs:read`, `fs:write` |
| `exec.svc` | Code execution + runtime discovery. | `exec:python`, `exec:js`, `exec:shell` |
| `mcp-bridge.svc` | MCP protocol adapter. | `fs:read` |
| `gateway.svc` | ATP WebSocket server. Ports 7700/7701. | `auth:session` |
| `gui.svc` | Browser UI server. | `fs:read`, `auth:session` |
| `shell.svc` | TTY interface. | `fs:read`, `fs:write`, `auth:session` |
| `llm.svc` | All AI inference. Owns all provider calls. | `llm:inference` |

**`llm.svc` is special:** `RuntimeExecutor` never calls provider APIs directly. All AI
inference goes through `llm.svc` via IPC (`llm/complete`, `llm/embed`, etc.).

---

## Service Identity

Services run as first-class kernel-managed processes with a `ServiceToken` (`AVIX_SVC_TOKEN`)
analogous to an agent's `CapabilityToken`. This token:

- Identifies the service in all IPC calls
- Scopes VFS writes to `/services/<name>/workspace/`
- Is issued at service start, held in memory, **never written to disk**

Services expose their current status in `/proc/services/<name>/status.yaml`.

---

## Multi-User Security — `_caller` Injection

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

Services that serve multiple users declare `caller_scoped: true` in `service.unit` and use
`_caller.user` to scope per-user behavior (e.g., resolve the correct credential from
`/secrets/alice/`).

**The kernel enforces tool ACLs before the call reaches the service** — unauthorized calls
never arrive. Services can trust `_caller` as authoritative.

---

## Tool Namespace — /tools/

`/tools/` contains only `.tool.yaml` descriptor files. No executable code lives here.
The in-memory registry (held by `tool-registry.svc`) reflects currently available state.

### Tool Descriptor Format

```yaml
name:        read
path:        /tools/fs/read
owner:       memfs.svc
description: Read file contents from the active storage backend.
status:
  state: available           # available | degraded | unavailable
  reason: null
ipc:
  transport: local-ipc
  endpoint:  memfs
  method:    fs.read
streaming:   false
job:         false
capabilities_required: [fs:read]
input:
  path: { type: string, required: true }
output:
  content: { type: string }
```

---

## Dynamic Tool Add/Remove

A service can add or remove tools at runtime (API availability, auth state, feature flags):

```json
// Add tools
{ "jsonrpc": "2.0", "method": "ipc.tool-add",
  "params": {
    "_token": "<svc_token>",
    "tools": [{ "name": "github/list-prs", "descriptor": {...}, "visibility": "all" }]
  }
}

// Remove tools
{ "jsonrpc": "2.0", "method": "ipc.tool-remove",
  "params": {
    "_token": "<svc_token>",
    "tools": ["github/list-prs"],
    "reason": "API unreachable",
    "drain": true
  }
}
```

`drain: true` waits for in-flight calls to complete before removing.

The kernel pushes a `tool.changed` ATP event to all subscribed clients when tools change.

**Tool states:** `available` | `degraded` | `unavailable`

**Category 2 tools** (`agent/`, `pipe/`, `cap/`, `job/`) are registered by `RuntimeExecutor`
at agent spawn via `ipc.tool-add` and deregistered at exit via `ipc.tool-remove`. They are
**never hard-coded** in any service's tool list.

---

## Service Installation

### Via ATP Command

```json
{
  "type": "cmd", "domain": "sys", "op": "install",
  "body": {
    "type": "service",
    "source": "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
    "checksum": "sha256:abc123..."
  }
}
```

Installation flow:

1. Download and verify checksum
2. Verify package signature
3. Conflict check (name, tool paths, ports)
4. Extract to `AVIX_ROOT/services/<name>/`
5. Write `service.unit`
6. Write `.install.json` receipt
7. Spawn process with env vars
8. `kernel/ipc/register`
9. `tool-registry.svc` rescan
10. Return `{ pid, tools[], status }`

---

## service.unit Format

```yaml
[service]
name:           github-svc
version:        1.2.0
description:    GitHub integration service
author:         github.com/example/github-svc

[exec]
command:        /services/github-svc/bin/github-svc
env:
  GITHUB_TOKEN_PATH: /secrets/{caller}/github-token.enc

[tools]
namespace:      github
manifest:       /services/github-svc/tools/

[limits]
max_concurrent: 20
queue_max:      100
queue_timeout:  5s

[deps]
after:          auth.svc, memfs.svc
requires:       auth.svc

[meta]
caller_scoped:  true
```

---

## Kernel Syscalls for Services

Services interact with the kernel via `kernel/ipc/` syscalls:

| Syscall | Description |
|---------|-------------|
| `kernel/ipc/register` | Register service at startup |
| `kernel/ipc/deregister` | Unregister on shutdown |
| `kernel/ipc/lookup` | Find another service's endpoint |
| `kernel/ipc/tool-add` | Add tools dynamically |
| `kernel/ipc/tool-remove` | Remove tools dynamically |

---

## Filesystem Ownership for Services

| Path | Owner | Agent writable? |
|------|-------|:--------------:|
| `/services/<name>/workspace/` | Service | Yes |
| `/services/<name>/bin/` | System (installer) | No |
| `/proc/services/<name>/status.yaml` | Kernel | No |

Services write their own workspace. Kernel writes the status file. Neither writes into
the other's tree.
