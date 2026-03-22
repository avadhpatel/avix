# Pipe

← [Back to Schema Index](./README.md)

**Kind:** `Pipe`  
**Location:** `/proc/<pid>/pipes/<pipe-id>.yaml` (written by kernel at runtime)  
**Direction:** Runtime (channel)

A unidirectional or bidirectional token-stream channel between two agents. Created in
response to a granted `pipe` [ResourceRequest](./resource-request.md) and destroyed when
either agent exits or explicitly closes it.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Pipe
metadata:
  pipeId: pipe-001
  createdAt: 2026-03-15T07:38:25-05:00
  createdBy: kernel

spec:
  sourcePid: 57
  targetPid: 58
  direction: out               # out (57→58) | in (58→57) | bidirectional
  bufferTokens: 16384
  backpressure: block          # block | drop | error — behaviour when buffer is full

  encoding: text               # text | json | yaml — payload format convention

status:
  state: open                  # open | closed | error
  tokensSent: 4200
  tokensAcknowledged: 4200
  closedAt: null
  closedReason: null
```

-----

## Backpressure Policies

|Policy |Behaviour when buffer is full                                        |
|-------|---------------------------------------------------------------------|
|`block`|Source agent blocks until target consumes; safe, may cause deadlock  |
|`drop` |Excess tokens are silently dropped; use only for lossy/streaming data|
|`error`|Kernel sends `SIGPIPE` to source agent; agent must handle            |

-----

## Pipe Lifecycle

1. Agent sends a `ResourceRequest` with `resource: pipe`
1. Kernel grants and writes Pipe record to `/proc/<sourcePid>/pipes/<pipeId>.yaml`
1. Both agents communicate over the channel
1. On agent exit or explicit close, kernel sets `status.state: closed` and sends
   `SIGPIPE` to the remaining agent

Intra-crew pipes (where `Crews.pipePolicy: allow-intra-crew`) skip steps 1–2 — the
kernel opens the channel directly without a ResourceRequest round-trip.

-----

## Related

- [ResourceRequest](./resource-request.md) — how pipes are requested
- [ResourceResponse](./resource-response.md) — grants include `pipeId` on success
- [Signal](./signal.md) — `SIGPIPE` is sent when a pipe partner closes
- [Crews](./crews.md) — `pipePolicy: allow-intra-crew` bypasses the request cycle

-----

## Field Defaults

|Field         |Default|Notes                             |
|--------------|-------|----------------------------------|
|`direction`   |`out`  |                                  |
|`bufferTokens`|`8192` |                                  |
|`backpressure`|`block`|Safest default; prevents data loss|
|`encoding`    |`text` |                                  |

System defaults at `/kernel/defaults/pipe.yaml`.
See [Resolved](./resolved.md) and [Defaults](./defaults.md).
