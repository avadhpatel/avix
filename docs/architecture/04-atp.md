# 04 — Avix Terminal Protocol (ATP)

> The external communication layer between clients and the Avix runtime.
> ATP never crosses inside — `gateway.svc` is the sole translator.

---

## Overview

ATP (Avix Terminal Protocol) is the **external** protocol for all client-to-Avix
communication. It runs over WebSocket with TLS.

```
EXTERNAL — clients ↔ Avix             INTERNAL — inside Avix
────────────────────────────          ─────────────────────────────────
ATP over WebSocket (TLS)              JSON-RPC 2.0 over local IPC sockets
Human users, apps, tooling            Services, agents, kernel
Authenticated via ATPToken            Authenticated via CapabilityToken / SvcToken
gateway.svc is the sole boundary      router.svc is the backbone
Long-lived, reconnectable             Fresh connection per call
```

**Key rule:** ATP never enters the system. `gateway.svc` is the only component that
speaks both protocols — it translates ATP commands into IPC tool calls and translates
IPC results back into ATP events. The internal world never speaks ATP.

---

## Endpoints

| Port | Purpose | Accessible by |
|------|---------|---------------|
| 7700 | User endpoint | Users with `user` role or above |
| 7701 | Admin endpoint | Users with `admin` or `operator` role |

Bind address is controlled by deployment mode:
- `localhost` for `gui` and `cli` modes
- `0.0.0.0` for `headless` (Docker / remote server)

---

## Authentication

Clients authenticate over ATP using an `ATPToken` (also called an API key or session token).
The token is presented in the WebSocket connection handshake or as a header.

Token format: `sk-avix-<32 base62 chars>` (~190 bits entropy).

`auth.svc` issues tokens. `gateway.svc` validates them on every inbound message.
The plaintext token is **never stored** — only the HMAC-SHA256 hash in `auth.conf`.

---

## Message Format

ATP messages are JSON objects with a fixed envelope:

### Client → Server (command)

```json
{
  "type": "cmd",
  "domain": "agent",
  "op": "spawn",
  "id": "req-001",
  "body": {
    "agent": "researcher",
    "goal": "Summarise Q3 earnings report"
  }
}
```

### Server → Client (event)

```json
{
  "type": "event",
  "domain": "agent",
  "op": "spawned",
  "requestId": "req-001",
  "body": {
    "pid": 57,
    "agent": "researcher",
    "status": "running"
  }
}
```

### Server → Client (error)

```json
{
  "type": "error",
  "requestId": "req-001",
  "code": "EPERM",
  "message": "Insufficient role to spawn agents"
}
```

---

## Command Domains

| Domain | Description | Minimum role |
|--------|-------------|--------------|
| `agent` | Spawn, kill, list, stat agents | `user` |
| `session` | Create, list, resume sessions | `user` |
| `hil` | Respond to human-in-loop events | `user` |
| `sys` | Install/uninstall services and agents | `operator` |
| `admin` | User management, key rotation | `admin` |
| `llm` | LLM provider status and config | `operator` |

---

## Key Commands

### Spawn an agent

```json
{
  "type": "cmd", "domain": "agent", "op": "spawn", "id": "req-001",
  "body": {
    "agent": "researcher",
    "goal": "Summarise the Q3 earnings report in /users/alice/workspace/q3.pdf",
    "capabilities": ["fs/read", "llm/complete"]
  }
}
```

### List running agents

```json
{ "type": "cmd", "domain": "agent", "op": "list", "id": "req-002", "body": {} }
```

### Kill an agent

```json
{ "type": "cmd", "domain": "agent", "op": "kill", "id": "req-003",
  "body": { "pid": 57, "reason": "Task complete" } }
```

### Respond to a HIL event

```json
{ "type": "cmd", "domain": "hil", "op": "respond", "id": "req-004",
  "body": {
    "hilId": "hil-001",
    "decision": "approved",
    "note": "Looks good, send it"
  }
}
```

### Install a service

```json
{
  "type": "cmd", "domain": "sys", "op": "install", "id": "req-005",
  "body": {
    "type": "service",
    "source": "https://pkg.avix.dev/github-svc-1.2.0.tar.gz",
    "checksum": "sha256:abc123..."
  }
}
```

---

## Key Events (Server → Client)

| Event | When emitted |
|-------|-------------|
| `agent.spawned` | Agent PID assigned and running |
| `agent.status` | Agent status changed (running/paused/completed/error) |
| `agent.output` | LLM turn output (streamed) |
| `hil.request` | Human input required (tool approval, escalation, capability upgrade) |
| `hil.timeout` | HIL request expired without response |
| `session.created` | New session established |
| `session.ended` | Session closed |
| `tool.changed` | Service added or removed a tool |
| `sys.installed` | Service/agent installation complete |

---

## HIL (Human-in-Loop) Flow

When an agent needs human approval, the full flow is:

```
1. Agent calls RuntimeExecutor with tool requiring approval
2. RuntimeExecutor sends ResourceRequest to kernel
3. Kernel mints ApprovalToken, writes /proc/<pid>/hil-queue/<hil-id>.yaml
4. Kernel sends SIGPAUSE to agent → agent suspends
5. Kernel pushes hil.request ATP event to all connected clients
6. Human client sends hil/respond command
7. gateway.svc translates → IPC → kernel validates ApprovalToken (single-use, atomic)
8. Kernel delivers SIGRESUME to agent with decision payload
9. Agent resumes
```

`ApprovalToken` is single-use. The first valid `hil/respond` atomically invalidates
all others. Subsequent `consume` attempts return `EUSED`.

---

## Connection Lifecycle

```
Client → WebSocket connect to :7700
         → Send auth header with ATPToken
         → gateway.svc validates token with auth.svc
         → Connection established

Client → Send cmd messages
         → gateway.svc translates to IPC calls
         → Receives events pushed by kernel/services

Disconnect → All pending subscriptions cleaned up
             → In-progress HIL events remain pending until hil_timeout
```

Long-lived connections are reconnectable. `gateway.svc` holds subscriptions per session
in `/proc/gateway/subscriptions.yaml`.
