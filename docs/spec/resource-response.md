# ResourceResponse

← [Back to Schema Index](./README.md)

**Kind:** `ResourceResponse`  
**Direction:** Kernel → Agent (IPC reply)

The kernel’s authoritative reply to a [ResourceRequest](./resource-request.md). Agents
must check `granted` on each item before using any resource — a partial grant is valid
and common.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: ResourceResponse
metadata:
  requestId: req-abc123
  respondedAt: 2026-03-15T07:38:25-05:00

spec:
  grants:
    - resource: context_tokens
      granted: true
      amount: 50000
      newTotal: 114000
      expiresAt: null             # null = for lifetime of session

    - resource: tool
      name: email
      granted: false
      reason: Requires human-in-the-loop approval
      suggestion: Send SIGPAUSE and present request to user via /proc/57/hil-queue

    - resource: pipe
      targetPid: 58
      granted: true
      pipeId: pipe-001
      direction: out
      bufferTokens: 16384

    - resource: token_renewal
      granted: true
      expiresAt: 2026-03-15T09:38:00-05:00
```

-----

## Notes

- Responses are **ordered to match** the `spec.requests` array in the originating
  ResourceRequest — the nth grant corresponds to the nth request.
- A `granted: false` item will always include a `reason` and, where applicable, a
  `suggestion` for how the agent should proceed.
- Agents should not retry a denied request in the same session without a changed
  context (e.g. after receiving `SIGRESUME` following human approval).

-----

## Related

- [ResourceRequest](./resource-request.md) — the originating request
- [Signal](./signal.md) — kernel sends `SIGRESUME` after a previously denied HIL request is approved
- [Pipe](./pipe.md) — a granted `pipe` resource creates a Pipe record at `/proc/<pid>/pipes/<pipeId>.yaml`
