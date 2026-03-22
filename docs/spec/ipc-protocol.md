# Avix IPC Protocol — Specification v1

> **Purpose:** Wire protocol for all internal communication between Avix processes —
> kernel, services, and agents. Completely separate from ATP (the external client protocol).
> **Audience:** Service authors, kernel contributors, SDK implementers.

-----

## Table of Contents

1. [Overview](#1-overview)
1. [Transport](#2-transport)
1. [Wire Format](#3-wire-format)
1. [Service Startup Contract](#4-service-startup-contract)
1. [Incoming Tool Calls](#5-incoming-tool-calls)
1. [Outgoing Calls](#6-outgoing-calls)
1. [Inbound Signals](#7-inbound-signals)
1. [Concurrency Model](#8-concurrency-model)
1. [Long-Running Jobs](#9-long-running-jobs)
1. [Error Codes](#10-error-codes)
1. [Reference Implementations](#11-reference-implementations)

-----

## 1. Overview

The IPC protocol is how every process inside Avix communicates. It uses JSON-RPC 2.0 over platform-native local sockets with a 4-byte length-prefix framing layer.

**Design goals:**

- Implementable in any language with only stdlib (no dependencies required)
- Single connection per call — natural concurrency without multiplexing complexity
- Identical wire format on all platforms

**Separation from ATP:**

```
ATP (external)                          IPC (internal)
──────────────────────────────          ─────────────────────────────────────
WebSocket over TLS                      Local sockets (Unix / Named Pipe)
Human users, apps, tooling              Services, agents, kernel
ATPToken (JWT-style)                    CapabilityToken / ServiceToken
gateway.svc is the entry point          router.svc is the backbone
Long-lived sessions                     Per-call connections
```

-----

## 2. Transport

### Platform Resolution

The kernel resolves the correct socket mechanism for the host platform. Service code uses logical names only via environment variables.

|Platform               |Mechanism            |Resolved path pattern  |
|-----------------------|---------------------|-----------------------|
|Linux                  |AF_UNIX domain socket|`/run/avix/<name>.sock`|
|macOS                  |AF_UNIX domain socket|`/run/avix/<name>.sock`|
|Windows ≥ 10 build 1803|Named Pipe           |`\\.\pipe\avix-<name>` |

Services never hard-code paths. They read `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, and `AVIX_SVC_SOCK` from environment variables which already contain the resolved OS path.

### Socket Layout

```
/run/avix/
├── kernel.sock           ← ResourceRequests and KernelSyscalls
├── router.sock           ← all tool calls route here first
├── auth.sock             ← token validation
├── memfs.sock            ← VFS operations
├── agents/
│   ├── 57.sock           ← signal delivery TO agent PID 57
│   └── 58.sock
└── services/
    ├── github-svc.sock
    └── web-search.sock
```

### Connection Model

The router opens a **fresh connection per tool call**. Services accept each connection independently. There is no persistent multiplexed connection between the router and a service. This gives services natural per-call concurrency without needing to implement connection management.

-----

## 3. Wire Format

Every message on the socket — in both directions, on all platforms — uses this framing:

```
┌─────────────────────────────────────────┐
│  4 bytes: payload length (uint32, LE)   │
├─────────────────────────────────────────┤
│  N bytes: UTF-8 JSON (JSON-RPC 2.0)     │
└─────────────────────────────────────────┘
```

**Read algorithm:**

1. Read exactly 4 bytes → parse as uint32 little-endian → `length`
1. Read exactly `length` bytes → parse as UTF-8 JSON

**Write algorithm:**

1. Encode message as UTF-8 JSON → `data`
1. Encode `len(data)` as uint32 little-endian → `header` (4 bytes)
1. Write `header + data` atomically

Implementations in common languages:

```python
# Python
import struct, socket, json

def send(sock, msg):
    data = json.dumps(msg).encode('utf-8')
    sock.sendall(struct.pack('<I', len(data)) + data)

def recv(sock):
    length = struct.unpack('<I', _read_exactly(sock, 4))[0]
    return json.loads(_read_exactly(sock, length))

def _read_exactly(sock, n):
    buf = b''
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk: raise ConnectionError("socket closed")
        buf += chunk
    return buf
```

```go
// Go
func send(conn net.Conn, msg interface{}) error {
    data, _ := json.Marshal(msg)
    header := make([]byte, 4)
    binary.LittleEndian.PutUint32(header, uint32(len(data)))
    _, err := conn.Write(append(header, data...))
    return err
}

func recv(conn net.Conn) (map[string]interface{}, error) {
    header := make([]byte, 4)
    if _, err := io.ReadFull(conn, header); err != nil { return nil, err }
    length := binary.LittleEndian.Uint32(header)
    body := make([]byte, length)
    if _, err := io.ReadFull(conn, body); err != nil { return nil, err }
    var msg map[string]interface{}
    return msg, json.Unmarshal(body, &msg)
}
```

```javascript
// Node.js
function send(socket, msg) {
  const data = Buffer.from(JSON.stringify(msg), 'utf8')
  const header = Buffer.allocUnsafe(4)
  header.writeUInt32LE(data.length, 0)
  socket.write(Buffer.concat([header, data]))
}
```

-----

## 4. Service Startup Contract

Every service — in any language — must complete this sequence before it is visible to the system.

### Environment Variables

```bash
AVIX_KERNEL_SOCK  # resolved OS path to kernel socket
AVIX_ROUTER_SOCK  # resolved OS path to router socket
AVIX_SVC_SOCK     # resolved OS path THIS service must listen on
AVIX_SVC_TOKEN    # service identity token (opaque string)
```

### Step 1 — Start Listening

Before registering, start listening on `AVIX_SVC_SOCK`. The kernel will start routing calls here once registration succeeds.

```python
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(os.environ['AVIX_SVC_SOCK'])
server.listen(128)
```

### Step 2 — Register with Kernel

Connect to `AVIX_KERNEL_SOCK` and send `ipc.register`:

```json
{
  "jsonrpc": "2.0",
  "id": "1",
  "method": "ipc.register",
  "params": {
    "token": "<AVIX_SVC_TOKEN>",
    "name": "my-svc",
    "endpoint": "<AVIX_SVC_SOCK>",
    "tools": ["my-svc/tool-a", "my-svc/tool-b"]
  }
}
```

Kernel response (success):

```json
{ "jsonrpc": "2.0", "id": "1", "result": { "registered": true, "pid": 23, "registry_version": 7 } }
```

Kernel response (failure — bad token):

```json
{ "jsonrpc": "2.0", "id": "1", "error": { "code": -32001, "message": "Invalid service token" } }
```

On error: exit immediately. A service with a rejected token has no valid identity.

### Step 3 — Accept Connections

Enter the accept loop. Handle each connection concurrently:

```python
while True:
    conn, _ = server.accept()
    threading.Thread(target=handle_connection, args=(conn,), daemon=True).start()
```

The service is now live and visible to the system.

-----

## 5. Incoming Tool Calls

Tool calls arrive as JSON-RPC requests on `AVIX_SVC_SOCK`. One request per connection — the connection closes after the response is sent.

### Request Shape

```json
{
  "jsonrpc": "2.0",
  "id": "call-abc123",
  "method": "my-svc/tool-a",
  "params": {
    "arg1": "value",
    "arg2": 42,
    "_caller": {
      "pid": 57,
      "user": "alice",
      "token": "eyJ..."
    }
  }
}
```

**`_caller` is always injected by the router.** The service must never trust caller identity from any other source. For `caller_scoped: true` services, `_caller.user` is the key used to scope per-user behavior (e.g., resolve the right secret from `/secrets/alice/`).

### Successful Response

```json
{
  "jsonrpc": "2.0",
  "id": "call-abc123",
  "result": {
    "output": "whatever the tool returns"
  }
}
```

### Error Response

```json
{
  "jsonrpc": "2.0",
  "id": "call-abc123",
  "error": {
    "code": -32005,
    "message": "GitHub API rate limit exceeded",
    "data": { "retry_after": 60 }
  }
}
```

After sending a response (success or error), close the connection.

-----

## 6. Outgoing Calls

Services make outbound calls by connecting to `AVIX_ROUTER_SOCK` (for tool calls to other services) or `AVIX_KERNEL_SOCK` (for kernel syscalls). Always include `_token` in params.

### Tool Call to Another Service

```json
{
  "jsonrpc": "2.0",
  "id": "out-001",
  "method": "fs/read",
  "params": {
    "path": "/services/my-svc/workspace/cache.json",
    "_token": "<AVIX_SVC_TOKEN>"
  }
}
```

### Kernel Syscall

```json
{
  "jsonrpc": "2.0",
  "id": "sys-001",
  "method": "kernel/proc/spawn",
  "params": {
    "_token": "<AVIX_SVC_TOKEN>",
    "agent": "researcher",
    "task": "run daily brief",
    "parent_pid": null
  }
}
```

### Dynamic Tool Registration

```json
// Add a tool at runtime
{
  "jsonrpc": "2.0",
  "id": "reg-001",
  "method": "ipc.tool-add",
  "params": {
    "_token": "<AVIX_SVC_TOKEN>",
    "tools": [{
      "name": "github/list-prs",
      "descriptor": { "description": "...", "input": {}, "output": {} },
      "visibility": "all"
    }]
  }
}

// Remove a tool at runtime
{
  "jsonrpc": "2.0",
  "id": "reg-002",
  "method": "ipc.tool-remove",
  "params": {
    "_token": "<AVIX_SVC_TOKEN>",
    "tools": ["github/list-prs"],
    "reason": "GitHub API unreachable",
    "drain": true
  }
}
```

`drain: true` — kernel waits for in-flight calls on this tool to complete before removing it.

-----

## 7. Inbound Signals

The kernel delivers signals to services as JSON-RPC **notifications** (no `id` field) on `AVIX_SVC_SOCK`. No response is expected or sent.

```json
{ "jsonrpc": "2.0", "method": "signal", "params": { "signal": "SIGHUP", "payload": {} } }
{ "jsonrpc": "2.0", "method": "signal", "params": { "signal": "SIGTERM", "payload": {} } }
```

**Required signal handling:**

|Signal   |Required action                                                       |
|---------|----------------------------------------------------------------------|
|`SIGHUP` |Re-read configuration; re-probe external connections                  |
|`SIGTERM`|Finish in-flight calls; emit `jobs.fail` for active jobs; exit cleanly|

**Agent-specific signals** (delivered on `/run/avix/agents/<pid>.sock`):

|Signal     |Meaning                      |
|-----------|-----------------------------|
|`SIGPAUSE` |Pause at next tool boundary  |
|`SIGRESUME`|Resume (carries HIL decision)|
|`SIGKILL`  |Terminate immediately        |
|`SIGSTOP`  |Session closed               |
|`SIGSAVE`  |Take snapshot now            |
|`SIGPIPE`  |Pipe established or closed   |

-----

## 8. Concurrency Model

### Per-Connection Independence

Each incoming connection is one tool call. Handle them independently:

```
service socket
  accept() → conn-1 → thread/goroutine/task → handle → respond → close
  accept() → conn-2 → thread/goroutine/task → handle → respond → close
  accept() → conn-3 → (queued — at max_concurrent)
```

The service does not need to know about other concurrent calls. No shared state is required between call handlers (beyond whatever the service’s own business logic requires).

### Backpressure

The router enforces concurrency limits declared in `service.unit`:

```toml
[service]
max_concurrent = 20    # max simultaneous open connections
queue_max      = 100   # max calls waiting for a slot
queue_timeout  = 5s    # queued call wait timeout
```

Calls beyond `queue_max` receive `EBUSY` immediately. Calls that wait longer than `queue_timeout` receive `ETIMEOUT`.

-----

## 9. Long-Running Jobs

Any tool that takes more than a few seconds should use the job pattern. The tool descriptor declares `job: true`.

### Contract

1. Service receives tool call
1. Service validates and starts background work
1. Service returns `{ "job_id": "job-7f3a9b" }` **immediately**
1. Connection closes
1. Background worker emits progress and completion events to `jobs.svc`

### Returning job_id

```json
{
  "jsonrpc": "2.0",
  "id": "call-abc",
  "result": { "job_id": "job-7f3a9b", "status": "running" }
}
```

### Emitting Progress

Connect to `AVIX_ROUTER_SOCK` and send notifications (no `id` — fire and forget):

```json
{ "jsonrpc": "2.0", "method": "jobs.emit",
  "params": { "_token": "<AVIX_SVC_TOKEN>", "job_id": "job-7f3a9b",
    "event": { "type": "progress", "percent": 45, "stage": "encoding" } } }
```

### Emitting Completion

```json
{ "jsonrpc": "2.0", "method": "jobs.complete",
  "params": { "_token": "<AVIX_SVC_TOKEN>", "job_id": "job-7f3a9b",
    "result": { "output_file": "/users/alice/workspace/result.mp4" } } }
```

### Emitting Failure

```json
{ "jsonrpc": "2.0", "method": "jobs.fail",
  "params": { "_token": "<AVIX_SVC_TOKEN>", "job_id": "job-7f3a9b",
    "error": { "code": -32001, "message": "Codec not available" } } }
```

### Job Event Types

|Type           |Required fields                 |Purpose               |
|---------------|--------------------------------|----------------------|
|`status_change`|none                            |Every state transition|
|`progress`     |`percent?`, `stage?`, `detail?` |Progress update       |
|`log`          |`stream` (stdout/stderr), `line`|Real-time output      |

### Job States

```
pending → running → done
                  → failed
running → paused → running
                → failed (on CANCEL)
```

-----

## 10. Error Codes

Standard JSON-RPC codes (-32700 to -32600) plus Avix-specific codes:

|Code  |Name            |Meaning                               |
|------|----------------|--------------------------------------|
|-32700|EPARSE          |JSON parse error                      |
|-32601|ENOTFOUND_METHOD|Method not found                      |
|-32602|EINVALID_PARAMS |Invalid params                        |
|-32001|EAUTH           |Bad or expired token                  |
|-32002|EPERM           |Capability not granted                |
|-32003|ENOTFOUND       |Resource doesn’t exist                |
|-32004|ELIMIT          |Rate limit / quota exceeded           |
|-32005|EUNAVAIL        |Tool unavailable (state: unavailable) |
|-32006|ECONFLICT       |Operation conflicts with current state|
|-32007|ETIMEOUT        |Call exceeded configured timeout      |
|-32008|EBUSY           |Service at max_concurrent capacity    |

-----

## 11. Reference Implementations

### Minimal Python Service (~50 lines)

```python
#!/usr/bin/env python3
import os, socket, json, struct, threading

KERNEL_SOCK = os.environ['AVIX_KERNEL_SOCK']
SVC_SOCK    = os.environ['AVIX_SVC_SOCK']
SVC_TOKEN   = os.environ['AVIX_SVC_TOKEN']

def send(sock, msg):
    data = json.dumps(msg).encode('utf-8')
    sock.sendall(struct.pack('<I', len(data)) + data)

def recv(sock):
    raw_len = b''
    while len(raw_len) < 4:
        raw_len += sock.recv(4 - len(raw_len))
    length = struct.unpack('<I', raw_len)[0]
    data = b''
    while len(data) < length:
        data += sock.recv(length - len(data))
    return json.loads(data.decode('utf-8'))

def handle(conn):
    try:
        req = recv(conn)
        if 'id' not in req:
            # notification (signal) — no response
            if req.get('params', {}).get('signal') == 'SIGTERM':
                os._exit(0)
            return
        method = req.get('method', '')
        if method == 'my-svc/hello':
            send(conn, {'jsonrpc': '2.0', 'id': req['id'],
                        'result': {'message': 'Hello from Python!'}})
        else:
            send(conn, {'jsonrpc': '2.0', 'id': req['id'],
                        'error': {'code': -32601, 'message': 'Method not found'}})
    finally:
        conn.close()

# Register with kernel
k = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
k.connect(KERNEL_SOCK)
send(k, {'jsonrpc': '2.0', 'id': '1', 'method': 'ipc.register',
         'params': {'token': SVC_TOKEN, 'name': 'my-python-svc',
                    'endpoint': SVC_SOCK, 'tools': ['my-svc/hello']}})
resp = recv(k)
assert resp.get('result', {}).get('registered'), f"Registration failed: {resp}"
k.close()

# Accept tool calls
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(SVC_SOCK)
server.listen(128)
while True:
    conn, _ = server.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
```

### Minimal Go Service (~80 lines)

```go
package main

import (
    "encoding/binary"
    "encoding/json"
    "io"
    "net"
    "os"
)

func sendMsg(conn net.Conn, msg interface{}) {
    data, _ := json.Marshal(msg)
    h := make([]byte, 4)
    binary.LittleEndian.PutUint32(h, uint32(len(data)))
    conn.Write(append(h, data...))
}

func recvMsg(conn net.Conn) map[string]interface{} {
    h := make([]byte, 4)
    io.ReadFull(conn, h)
    body := make([]byte, binary.LittleEndian.Uint32(h))
    io.ReadFull(conn, body)
    var msg map[string]interface{}
    json.Unmarshal(body, &msg)
    return msg
}

func handle(conn net.Conn) {
    defer conn.Close()
    req := recvMsg(conn)
    id, hasID := req["id"]
    if !hasID { return } // signal notification
    method := req["method"].(string)
    if method == "my-svc/hello" {
        sendMsg(conn, map[string]interface{}{
            "jsonrpc": "2.0", "id": id,
            "result": map[string]string{"message": "Hello from Go!"},
        })
    } else {
        sendMsg(conn, map[string]interface{}{
            "jsonrpc": "2.0", "id": id,
            "error": map[string]interface{}{"code": -32601, "message": "not found"},
        })
    }
}

func main() {
    // Register
    k, _ := net.Dial("unix", os.Getenv("AVIX_KERNEL_SOCK"))
    sendMsg(k, map[string]interface{}{
        "jsonrpc": "2.0", "id": "1", "method": "ipc.register",
        "params": map[string]interface{}{
            "token": os.Getenv("AVIX_SVC_TOKEN"), "name": "my-go-svc",
            "endpoint": os.Getenv("AVIX_SVC_SOCK"), "tools": []string{"my-svc/hello"},
        },
    })
    recvMsg(k); k.Close()

    // Listen
    l, _ := net.Listen("unix", os.Getenv("AVIX_SVC_SOCK"))
    for {
        conn, _ := l.Accept()
        go handle(conn)
    }
}
```
