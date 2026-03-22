# Avix Service Authoring Guide v1

> **Purpose:** Everything a service author needs to build and install a service in any language.
> **Prerequisite:** Read [IPC_Protocol.md](./IPC_Protocol.md) for the wire protocol details.

-----

## Table of Contents

1. [What is a Service?](#1-what-is-a-service)
1. [service.unit Reference](#2-serviceunit-reference)
1. [The Four Things Every Service Does](#3-the-four-things-every-service-does)
1. [Handling Concurrent Calls](#4-handling-concurrent-calls)
1. [Implementing Long-Running Tools](#5-implementing-long-running-tools)
1. [Dynamic Tool Management](#6-dynamic-tool-management)
1. [Multi-User / caller_scoped Services](#7-multi-user--caller_scoped-services)
1. [Secrets Access](#8-secrets-access)
1. [Packaging and Installation](#9-packaging-and-installation)
1. [Testing Your Service](#10-testing-your-service)

-----

## 1. What is a Service?

A service is a long-running OS process that:

- Exposes tools at a `/tools/<namespace>/` path
- Speaks JSON-RPC 2.0 over a platform-native local socket
- Runs alongside the Avix kernel as a peer, not inside it

Services are written in **any language**. Rust, Python, Go, Node, Ruby — anything that can open a socket and parse JSON.

### Service vs Agent

|           |Service                        |Agent                       |
|-----------|-------------------------------|----------------------------|
|Logic type |Deterministic                  |LLM-driven                  |
|Lifecycle  |Long-running, always available |Spawned per task            |
|Language   |Any                            |Rust (RuntimeExecutor) + LLM|
|Host access|Yes — network, filesystem, etc.|Only via tools              |
|Startup    |Phase 4 (before agents)        |On demand                   |

### What services are good for

- Wrapping external APIs (GitHub, Slack, database connectors)
- Providing host tools to agents (code execution, file operations)
- Long-running background work (indexing, monitoring, syncing)
- Bridging external protocols (MCP, webhooks, queues)

-----

## 2. service.unit Reference

The `service.unit` file is the service’s manifest. It lives at `AVIX_ROOT/services/<n>/service.unit`.

```toml
# Identity
name        = github-svc            # unique name, kebab-case
version     = 1.2.0
source      = community             # system | community | user
signature   = sha256:abc...         # package signature hash

[unit]
description   = GitHub integration service
requires      = [router, auth, memfs]   # services that must be running first
after         = [auth]                  # start order (subset of requires)

[service]
binary        = /services/github-svc/bin/github-svc  # executable path
language      = go               # informational only (rust|python|go|node|any)
restart       = on-failure       # on-failure | always | never
restart_delay = 5s

# Concurrency limits (router enforces these)
max_concurrent = 20              # max simultaneous open connections
queue_max      = 100             # max calls queued waiting for a slot
queue_timeout  = 5s              # queued call timeout → ETIMEOUT

# Identity scope
run_as = service                 # service (default) | user:<username>
                                 # service: VFS writes scoped to /services/<n>/
                                 # user:<n>: runs under that user's identity (single-user only)

[capabilities]
required     = [fs:read, fs:write]
scope        = /services/github-svc/    # VFS write scope
host_access  = [network]               # network | filesystem:<path> | socket:<path> | env:<VAR>
caller_scoped = true                   # inject _caller on every tool call

[tools]
namespace = /tools/github/
provides  = [list-prs, create-issue, search-code, get-file]
# Tools listed here are the maximum possible surface.
# Services can remove/add subsets at runtime via ipc.tool-add / ipc.tool-remove.

[jobs]
max_active  = 3        # max simultaneous background jobs
job_timeout = 3600s    # kernel marks job failed after this
persist     = false    # true = job survives service restart
```

### host_access values

|Value              |What it grants                            |
|-------------------|------------------------------------------|
|`network`          |Outbound TCP/UDP connections              |
|`filesystem:<path>`|Direct host filesystem access at this path|
|`socket:<path>`    |Connect to a Unix socket at this path     |
|`env:<VAR>`        |Read this environment variable at startup |

`host_access` is validated at install time. The operator approves the declared access. Undeclared host access is not enforced by the kernel (advisory model) but is auditable.

-----

## 3. The Four Things Every Service Does

### 1. Read environment

```python
import os
KERNEL_SOCK = os.environ['AVIX_KERNEL_SOCK']  # kernel socket path
ROUTER_SOCK = os.environ['AVIX_ROUTER_SOCK']  # router socket path
SVC_SOCK    = os.environ['AVIX_SVC_SOCK']     # YOUR socket to listen on
SVC_TOKEN   = os.environ['AVIX_SVC_TOKEN']    # your identity token
```

### 2. Start listening BEFORE registering

Start listening on `AVIX_SVC_SOCK` first. The kernel will start routing calls the moment registration succeeds, so the socket must be ready.

### 3. Register with kernel

```json
→ AVIX_KERNEL_SOCK
{
  "jsonrpc": "2.0", "id": "1", "method": "ipc.register",
  "params": {
    "token": "<SVC_TOKEN>",
    "name": "github-svc",
    "endpoint": "<SVC_SOCK>",
    "tools": ["github/list-prs", "github/create-issue"]
  }
}

← response must have result.registered == true
   on error: exit immediately
```

### 4. Accept and handle connections concurrently

```python
while True:
    conn, _ = server.accept()
    # spawn thread / goroutine / async task per connection
    handle_in_background(conn)
```

Each connection is one tool call. Handle → respond → close. See [IPC_Protocol.md §4-5](./IPC_Protocol.md) for message shapes.

-----

## 4. Handling Concurrent Calls

Each incoming connection is independent. The simplest approach in each language:

```python
# Python — thread per connection
import threading
while True:
    conn, _ = server.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
```

```go
// Go — goroutine per connection
for {
    conn, _ := listener.Accept()
    go handle(conn)
}
```

```javascript
// Node — event loop handles it
server.on('connection', conn => handle(conn))
```

```rust
// Rust/Tokio — task per connection
loop {
    let (conn, _) = listener.accept().await?;
    tokio::spawn(handle(conn));
}
```

The router enforces `max_concurrent` from `service.unit` — your service never sees more than this many simultaneous connections. Beyond that, the router queues or rejects. You don’t need to implement back-pressure.

-----

## 5. Implementing Long-Running Tools

For tools that take more than a few seconds, use the job pattern.

### 1. Declare in tool descriptor

```yaml
# /services/my-svc/tools/video/transcode.tool.yaml
name: video/transcode
job: true
job_timeout: 3600s
streaming: true
```

### 2. Return job_id immediately

```python
def handle_transcode(req, conn):
    params = req['params']
    job_id = f"job-{uuid4().hex[:8]}"
    
    # Start background work
    threading.Thread(
        target=transcode_worker,
        args=(job_id, params['file'], params['format']),
        daemon=True
    ).start()
    
    # Return immediately — connection closes
    send(conn, {
        "jsonrpc": "2.0", "id": req['id'],
        "result": { "job_id": job_id, "status": "running" }
    })
```

### 3. Background worker emits events

```python
def transcode_worker(job_id, input_file, fmt):
    router = connect_to(ROUTER_SOCK)
    
    try:
        for progress in do_transcode(input_file, fmt):
            send(router, {
                "jsonrpc": "2.0",
                "method": "jobs.emit",
                "params": {
                    "_token": SVC_TOKEN,
                    "job_id": job_id,
                    "event": { "type": "progress", "percent": progress }
                }
            })
        
        send(router, {
            "jsonrpc": "2.0",
            "method": "jobs.complete",
            "params": {
                "_token": SVC_TOKEN,
                "job_id": job_id,
                "result": { "output": f"/users/alice/workspace/output.{fmt}" }
            }
        })
    except Exception as e:
        send(router, {
            "jsonrpc": "2.0",
            "method": "jobs.fail",
            "params": {
                "_token": SVC_TOKEN,
                "job_id": job_id,
                "error": { "code": -32001, "message": str(e) }
            }
        })
    finally:
        router.close()
```

Job notifications are fire-and-forget — no response is expected.

-----

## 6. Dynamic Tool Management

Services can add and remove tools at runtime without restarting.

### Adding tools (e.g., external API came online)

```python
def on_github_reconnected():
    router = connect_to(ROUTER_SOCK)
    send(router, {
        "jsonrpc": "2.0", "id": "add-1", "method": "ipc.tool-add",
        "params": {
            "_token": SVC_TOKEN,
            "tools": [{
                "name": "github/list-prs",
                "descriptor": {
                    "description": "List open pull requests",
                    "input": { "repo": { "type": "string", "required": True } },
                    "output": { "prs": { "type": "array" } }
                },
                "visibility": "all"    # all | crew:<name> | user:<name>
            }]
        }
    })
    recv(router)  # wait for confirmation
    router.close()
```

### Removing tools (e.g., auth lost, API down)

```python
def on_github_auth_failed():
    router = connect_to(ROUTER_SOCK)
    send(router, {
        "jsonrpc": "2.0", "id": "rem-1", "method": "ipc.tool-remove",
        "params": {
            "_token": SVC_TOKEN,
            "tools": ["github/list-prs", "github/create-issue"],
            "reason": "OAuth token revoked",
            "drain": True   # wait for in-flight calls to complete
        }
    })
    recv(router)
    router.close()
```

### Tool visibility scoping

```python
# Only alice can use this tool
"visibility": "user:alice"

# Only members of the researchers crew
"visibility": "crew:researchers"

# Everyone who has the github/* capability
"visibility": "all"
```

### Responding to SIGHUP

`SIGHUP` is the standard “re-check your connections” signal. Good place to re-probe external APIs and update tool availability:

```python
def handle_signal(req, conn):
    if req['params']['signal'] == 'SIGHUP':
        # Re-probe GitHub API
        threading.Thread(target=re_probe_and_update_tools, daemon=True).start()
    elif req['params']['signal'] == 'SIGTERM':
        graceful_shutdown()
```

-----

## 7. Multi-User / caller_scoped Services

When `caller_scoped: true` in `service.unit`, the router injects `_caller` into every tool call params. Use it to scope per-user behavior.

```python
def handle_list_prs(req, conn):
    caller = req['params']['_caller']
    username = caller['user']  # e.g., "alice"
    
    # Get alice's GitHub token from secrets
    token = get_secret_for_user(username, 'github-token')
    
    # Make API call with alice's credentials
    prs = github_api.list_prs(
        token=token,
        repo=req['params']['repo']
    )
    
    send(conn, {"jsonrpc": "2.0", "id": req['id'], "result": {"prs": prs}})
```

### Getting user secrets

Services request secrets via the kernel, not by reading `/secrets/` directly (that path is inaccessible via VFS):

```python
def get_secret_for_user(username, secret_name):
    kernel = connect_to(KERNEL_SOCK)
    send(kernel, {
        "jsonrpc": "2.0", "id": "sec-1", "method": "kernel/secret/get",
        "params": {
            "_token": SVC_TOKEN,
            "owner": f"user:{username}",
            "name": secret_name
        }
    })
    resp = recv(kernel)
    kernel.close()
    return resp['result']['value']
```

The kernel validates the service token, checks that this service has been granted access to secrets for this user, decrypts the blob, and returns the plaintext — only in the response, never written anywhere.

-----

## 8. Secrets Access

Services can also have their own secrets (e.g., a shared API key for a service-level integration):

```bash
# Set a secret for a service
avix secret set github-app-key "ghp_..." --for-service github-svc
```

Access from service code:

```python
kernel = connect_to(KERNEL_SOCK)
send(kernel, {
    "jsonrpc": "2.0", "id": "sec-1", "method": "kernel/secret/get",
    "params": { "_token": SVC_TOKEN, "owner": "service:github-svc", "name": "github-app-key" }
})
```

-----

## 9. Packaging and Installation

### Package structure

```
github-svc-1.2.0/
├── service.unit           # required
├── bin/
│   └── github-svc         # executable (or github-svc.exe on Windows)
├── tools/
│   ├── github-list-prs.tool.yaml
│   ├── github-create-issue.tool.yaml
│   └── github-get-file.tool.yaml
└── README.md
```

Package as a signed tarball:

```bash
tar czf github-svc-1.2.0.tar.gz github-svc-1.2.0/
sha256sum github-svc-1.2.0.tar.gz > github-svc-1.2.0.tar.gz.sha256
# Sign with your key
```

### Installing via ATP

```json
{
  "type": "cmd", "domain": "sys", "op": "install", "token": "<admin_token>",
  "body": {
    "type": "service",
    "source": "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
    "checksum": "sha256:abc123..."
  }
}
```

### Installing via CLI

```bash
avix service install ./github-svc-1.2.0.tar.gz
# or from URL:
avix service install https://pkg.avix.dev/github-svc-1.2.0.tar.gz
```

### What happens at install time

1. Download and verify checksum
1. Verify package signature
1. Conflict check: name, tool namespace, ports
1. Extract to `AVIX_ROOT/services/github-svc/`
1. Write `.install.json` receipt
1. If `autostart: true` (default): spawn the process

The service persists across reboots automatically — the `service.unit` file is what the kernel reads at Phase 4.

-----

## 10. Testing Your Service

### Unit testing (no Avix needed)

Test your tool handler functions in isolation. They receive a JSON dict and return a JSON dict — pure functions are easy to test.

### Integration testing with mock kernel

Use the Avix test harness (or implement a simple mock):

```python
# Start a mock kernel that accepts ipc.register
# and forwards test tool calls to your service
mock_kernel = MockKernel(port='/tmp/test-kernel.sock')
mock_kernel.start()

# Set env vars pointing to mock
os.environ['AVIX_KERNEL_SOCK'] = '/tmp/test-kernel.sock'
os.environ['AVIX_SVC_SOCK'] = '/tmp/test-svc.sock'
os.environ['AVIX_SVC_TOKEN'] = 'test-token'

# Start your service
proc = subprocess.Popen(['python', 'main.py'])

# Send a test tool call
result = mock_kernel.call('github/list-prs', {'repo': 'org/repo'}, caller={'user': 'alice'})
assert result['prs'] is not None

proc.terminate()
```

### End-to-end testing

Install the service into a test Avix instance and use ATP commands to invoke it through the full stack:

```bash
# Start test Avix
avix config init --root /tmp/test-avix --user test --credential-type api_key
avix install --root /tmp/test-avix
avix start --root /tmp/test-avix &

# Install your service
avix service install ./my-svc.tar.gz

# Test via ATP
curl -X POST wss://localhost:7700/atp ...
```
