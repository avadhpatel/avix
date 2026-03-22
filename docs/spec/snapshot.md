# Snapshot

← [Back to Schema Index](./README.md)

**Kind:** `Snapshot`  
**Location:** `/users/<username>/snapshots/<agent>-<timestamp>.yaml` or `/services/<svcname>/snapshots/<agent>-<timestamp>.yaml`  
**Direction:** Persistence

A point-in-time serialisation of a running agent’s full state. Created on `SIGSAVE`,
at `autoSnapshotInterval`, or manually by a user command. Used for crash recovery and
agent cloning.

Snapshots are stored in the owner’s persistent workspace tree — they survive reboots
and can be migrated to another Avix instance alongside the rest of `/users/<username>/`.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Snapshot
metadata:
  name: researcher-20260315-0741
  agentName: researcher
  sourcePid: 57
  capturedAt: 2026-03-15T07:41:00-05:00
  capturedBy: kernel            # kernel | user:<uid> | agent:<pid>
  trigger: auto                 # auto | crash | manual | sigsave

spec:
  goal: "Research quantum computing breakthroughs 2025"
  contextSummary: |
    Agent has completed web search phase. Found 12 sources.
    Currently synthesising findings. 3 tool calls remaining in chain.
  contextTokenCount: 64000

  memory:
    episodicEvents: 14
    semanticKeys: 8

  pendingRequests:
    - requestId: req-abc124
      resource: tool
      name: web
      status: in-flight         # in-flight requests are re-issued on restore

  pipes:
    - pipeId: pipe-001
      state: open               # restored pipes are reconnected if target still running

  environment:
    temperature: 0.7
    capabilityToken: sha256:tokenSig789...

  checksum: sha256:snap001...   # integrity check verified by kernel on restore
```

-----

## Trigger Types

|Trigger  |Cause                                                                      |
|---------|---------------------------------------------------------------------------|
|`auto`   |Periodic snapshot via `AgentManifest.spec.snapshot.autoSnapshotIntervalSec`|
|`crash`  |Kernel detected unexpected termination; captured last known state          |
|`manual` |User ran `avix snapshot <pid>`                                             |
|`sigsave`|Kernel sent `SIGSAVE` signal to agent                                      |

## Restore Behaviour

- **Pending requests** with `status: in-flight` are re-issued to the kernel automatically.
- **Pipes** with `state: open` are reconnected if the target agent is still running;
  otherwise the restored agent receives `SIGPIPE`.
- The `checksum` is verified by the kernel before restore; a mismatch aborts the restore
  and logs an integrity error.
- A fresh `CapabilityToken` is issued on restore — the snapshotted token is used only to
  determine the original capability set, not reused directly.

-----

## Related

- [AgentManifest](./agent-manifest.md) — `spec.snapshot` configures auto-snapshot and restore-on-crash
- [Signal](./signal.md) — `SIGSAVE` and `SIGKILL`+crash trigger snapshot creation
- [Pipe](./pipe.md) — open pipes at snapshot time are restored if possible

-----

## Field Defaults

|Field    |Default |Notes                                     |
|---------|--------|------------------------------------------|
|`trigger`|`manual`|When not set, assumed to be user-initiated|

Snapshot creation is governed by `AgentManifest.spec.snapshot` defaults.
System defaults at `/kernel/defaults/agent-manifest.yaml` under `snapshot:`.
See [Defaults](./defaults.md).
