# 05 — Capabilities

> CapabilityToken, token lifecycle, HIL escalation, role hierarchy, and session model.

---

## Overview

The capability system is the trust boundary in Avix. Every agent action is gated by a
`CapabilityToken` issued by the kernel at spawn. The token:

- Lists exactly which tools the agent may call (`spec.tools.granted`)
- Is HMAC-signed — any modification invalidates it
- Has a time-limited expiry with automatic renewal
- Is enforced by the kernel at every tool dispatch

**ADR-01:** Tools are the security boundary. `CapabilityToken.granted_tools` is the single
source of truth. A separate coarse-grained capabilities layer is redundant.

---

## CapabilityToken Schema

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
    granted: [fs/read, llm/complete, web/search]
  constraints:
    maxTokensPerTurn: 8000
    maxToolChainLength: 8
    allowPipeTargets: [58]
    tool_call_budget: {}          # per-tool call limits; { send_email: 1 } = once-only
  signature: sha256:tokenSig789...  # HMAC-signed; any modification invalidates
```

Key rules:
- `spec.tools.granted` lists **only** tools actually granted — absent tools require HIL
- Tokens are HMAC-signed — agents treat them as opaque strings, never parse internals
- New tokens from capability upgrades carry the **same `expiresAt`** as the replaced token
- `tool_call_budget` is enforced by the kernel at dispatch, not by the agent

---

## Token Lifecycle

```
spawn → CapabilityToken issued (tools from crew + user ACL intersection)
  │
  ├── normal operation: agent presents token on every tool call
  │
  ├── token_renewal: agent sends ResourceRequest { resource: token_renewal }
  │     kernel issues fresh token, same grants, new expiry + signature
  │     auto-approved — always succeeds within quota
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

---

## ResourceRequest — Requesting More Capability

Agents send `ResourceRequest` to `AVIX_KERNEL_SOCK` when they need tools not in their token:

```yaml
apiVersion: avix/v1
kind: ResourceRequest
metadata:
  agentPid: 57
  requestId: req-abc123
  capabilityToken: sha256:tokenSig789...
spec:
  requests:
    # Tool NOT in token → triggers HIL capability_upgrade
    - resource: tool
      name: send_email
      reason: "Need to notify user when analysis complete"
      urgency: low

    # Tool IS in token but policy requires per-call approval
    - resource: tool_call_approval
      tool: send_email
      args: { to: "team@org.com", subject: "Summary ready" }
      reason: "Sending research summary"

    # Standard token renewal (always auto-approved)
    - resource: token_renewal
      reason: Token expires in 5 minutes

    # Context window expansion
    - resource: context_tokens
      amount: 50000
      reason: Need longer research thread
```

### ResourceResponse

```yaml
apiVersion: avix/v1
kind: ResourceResponse
metadata:
  requestId: req-abc123
spec:
  grants:
    - resource: tool
      name: send_email
      granted: false
      hil_pending: true           # SIGPAUSE sent; agent must wait for SIGRESUME
      hilId: hil-002

    - resource: token_renewal
      granted: true
      expiresAt: 2026-03-15T09:38:00-05:00

    - resource: context_tokens
      granted: true
      amount: 50000
      newTotal: 114000
```

`hil_pending: true` means: kernel has opened a HIL event, sent `SIGPAUSE` to the agent,
and pushed a `hil.request` ATP event. The agent is suspended until `SIGRESUME`.

---

## HIL Scenarios

### Scenario 1 — Tool Call Approval

Agent calls `send_email` (in token, but `hilRequiredTools` list in `kernel.yaml`):

1. RuntimeExecutor intercepts before dispatching
2. Sends `ResourceRequest { resource: tool_call_approval, tool: send_email, args: {...} }`
3. Kernel: `SIGPAUSE` → agent, `hil.request` ATP event → client
4. Human: approves or denies via `hil/respond`
5. Kernel: `SIGRESUME { decision: "approved" }` → agent proceeds

### Scenario 2 — Capability Upgrade

Agent calls `bash` (not in token):

1. RuntimeExecutor sends `ResourceRequest { resource: tool, name: bash }`
2. Kernel: `SIGPAUSE` → agent, `hil.request` ATP event → client
3. Human: approves with scope `session` or `once`
4. Kernel: issues new token, `SIGRESUME { new_capability_token: "..." }` → agent
5. RuntimeExecutor replaces its held token

### Scenario 3 — SIGESCALATE (Agent-Initiated)

Agent encounters ambiguity and proactively requests human guidance:

```json
{ "signal": "SIGESCALATE",
  "payload": {
    "reason": "Found PII in dataset. Unsure whether to include.",
    "context": "Researching Q3 financials, found employee salary data...",
    "options": [
      { "id": "include", "label": "Include with redaction" },
      { "id": "exclude", "label": "Exclude entirely" }
    ]
  }
}
```

Kernel: mints `ApprovalToken`, writes HIL record, pushes `hil.request { type: escalation }`.
Agent is already suspended (it sent the signal).

---

## ApprovalToken

HIL escalation mints one `ApprovalToken` per event, broadcast to all connected `human_channel`
tools simultaneously. The first valid response atomically invalidates all others.
Subsequent `consume` attempts return `EUSED`.

---

## Role Hierarchy

```
admin    ← full system control
  operator  ← spawn/kill agents, manage services, view all logs
    user     ← spawn own agents, manage own workspace
      guest  ← read-only
```

Roles are set in `auth.conf` per identity. Role is checked by the kernel before any
tool dispatch — roles are the outer gate, capability tokens are the inner gate.

---

## Session Model

### Core Principle

`/etc/avix/auth.conf` survives restarts. Active tokens do not. Tokens are always freshly
derived from policy at session start.

### Session Lifecycle

```
client → ATP auth (api_key or password)
       → auth.svc validates credential
       → issues ATPToken (session token)
       → session entry written to redb (persistent) AND
         /proc/users/<username>/sessions/<sid>.yaml (VFS — ephemeral view)

session active:
  → ATPToken used on every ATP message
  → agent spawns get CapabilityToken derived from user ACL + crew membership

session end (logout or TTL expiry):
  → redb entry updated to "completed"
  → VFS manifest at /proc/users/<username>/sessions/<sid>.yaml removed
```

### Session Manifest VFS Schema

```yaml
apiVersion: avix/v1
kind: SessionManifest
metadata:
  sessionId: sess-abc-123
  username: alice
  createdAt: 2026-03-22T10:00:00Z
  updatedAt: 2026-03-22T10:05:30Z
spec:
  agentName: researcher
  goal: "Research Q3 revenue trends"
  status: active       # active | completed | error
  messageCount: 7
```

This manifest is:
- Written by `SessionStore` on every `save()` when a VFS handle is attached
- Removed by `SessionStore` on `delete()`
- Read-only for agents (protected by `is_agent_writable()`)
- Ephemeral — not persisted across reboots (VFS is in-memory)
- The redb store is the source of truth for durability

### Services Receive ServiceToken

Services receive an analogous `ServiceToken` (`AVIX_SVC_TOKEN`) at startup, scoped to
`/services/<name>/workspace/`. Issued at service start, held in memory, never written to disk.
