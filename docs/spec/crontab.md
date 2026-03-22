# Crontab

← [Back to Schema Index](./README.md)

**Kind:** `Crontab`  
**Location:** `/etc/avix/crontab.yaml`  
**Direction:** Config (static)

Defines scheduled agent invocations. Uses standard 5-field cron expressions (UTC by
default). The kernel spawns a **fresh agent instance** per job run — jobs do not reuse
a persistent agent.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  timezone: UTC                # default; override per-job with job.timezone

  jobs:
    - id: memory-gc-daily
      schedule: "0 3 * * *"   # daily at 03:00 UTC
      user: svc-memory-gc
      agentTemplate: memory-gc
      goal: Compact episodic memory older than 7 days
      args:
        retentionDays: 7
      onFailure: alert         # ignore | alert | retry

    - id: pipeline-hourly
      schedule: "0 * * * *"   # every hour
      user: svc-pipeline
      agentTemplate: pipeline-ingest
      goal: Ingest and summarize latest data from configured sources
      timeout: 1800            # seconds; kernel sends SIGSTOP if exceeded
      onFailure: retry
      retryPolicy:
        maxAttempts: 3
        backoffSec: 60
```

-----

## Field Reference

|Field          |Required|Description                                                   |
|---------------|--------|--------------------------------------------------------------|
|`id`           |Yes     |Unique job identifier; used in logs and alerts                |
|`schedule`     |Yes     |Standard 5-field cron expression                              |
|`user`         |Yes     |Username under whose quota and tool permissions the agent runs|
|`agentTemplate`|Yes     |Name of the agent manifest to spawn                           |
|`goal`         |Yes     |Goal string passed to the agent at spawn                      |
|`args`         |No      |Key-value pairs merged into the agent’s goal template vars    |
|`timeout`      |No      |Max wall-clock seconds; kernel sends `SIGSTOP` if exceeded    |
|`onFailure`    |No      |`ignore` | `alert` | `retry` (default: `alert`)               |
|`retryPolicy`  |No      |Required when `onFailure: retry`                              |

-----

## Related

- [AgentManifest](./agent-manifest.md) — `agentTemplate` references a manifest by `metadata.name`
- [Users](./users.md) — job runs under `user`’s quota and tool permissions
- [Signal](./signal.md) — `SIGSTOP` is sent on timeout; `SIGSAVE` may be sent before stop

-----

## Field Defaults

|Field                           |Default|Notes                                     |
|--------------------------------|-------|------------------------------------------|
|`spec.timezone`                 |`UTC`  |                                          |
|`jobs[].timeout`                |`3600` |1 hour; kernel sends `SIGSTOP` if exceeded|
|`jobs[].onFailure`              |`alert`|                                          |
|`jobs[].retryPolicy.maxAttempts`|`3`    |Only applies when `onFailure: retry`      |
|`jobs[].retryPolicy.backoffSec` |`60`   |                                          |

System defaults at `/kernel/defaults/crontab.yaml`.
See [Resolved](./resolved.md) and [Defaults](./defaults.md).
