# AgentStatus

← [Back to Schema Index](./README.md)

**Kind:** `AgentStatus`  
**Location:** `/proc/<pid>/status.yaml`  
**Direction:** Status (dynamic)

Written by the kernel into `/proc/<pid>/status.yaml` at runtime. Agents and users may
read this; only the kernel may write it.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: AgentStatus
metadata:
  name: researcher
  pid: 57
  spawnedAt: 2026-03-15T07:38:00-05:00
  spawnedBy: alice          # username or pid of parent agent

status:
  state: running            # pending | running | paused | waiting | stopped | crashed
  goal: "Research quantum computing breakthroughs 2025"
  contextUsed: 64000        # tokens currently in working context
  contextLimit: 200000
  toolCallsThisTurn: 3
  lastActivityAt: 2026-03-15T07:41:12-05:00
  waitingOn: null           # null | human-approval | pipe-read | pipe-write | signal

  tools:
    granted:
      - web_search
      - web_fetch
      - file_read
    denied:
      - send_email          # optional tool denied at spawn — not in user's crew

  pipes:
    - id: pipe-001
      targetPid: 58
      direction: out
      state: open

  signals:
    lastReceived: null
    pendingCount: 0

  metrics:
    tokensConsumed: 14200   # this session
    toolCallsTotal: 11
    wallTimeSec: 192
```

-----

## State Reference

|State    |Meaning                                                            |
|---------|-------------------------------------------------------------------|
|`pending`|Spawned but not yet started; waiting for kernel resource allocation|
|`running`|Actively executing goal                                            |
|`paused` |Suspended by `SIGPAUSE`; consuming no resources                    |
|`waiting`|Blocked on an external event (see `waitingOn`)                     |
|`stopped`|Gracefully shut down via `SIGSTOP`                                 |
|`crashed`|Terminated unexpectedly; kernel may restore from snapshot          |

-----

## Related

- [AgentManifest](./agent-manifest.md) — static definition this status reflects
- [Signal](./signal.md) — events that drive state transitions
- [Pipe](./pipe.md) — detail on pipe entries in `status.pipes`
- [Snapshot](./snapshot.md) — created on crash or `SIGSAVE`; used for restore
