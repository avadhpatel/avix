# SessionManifest

← [Back to Schema Index](./README.md)

**Kind:** `SessionManifest`  
**Location:** `/proc/users/<username>/sessions/<session-id>.yaml`  
**Direction:** Status (ephemeral — lost on reboot)

Tracks an active session — a user-facing context in which one or more agents may run.
Sessions correspond to shell logins, API connections, or UI conversations.

Sessions are **runtime state**, not user data. They live in `/proc/` alongside other
kernel-generated views. When Avix reboots or a connection drops, the session record is
gone. There is no session history or audit trail in the VFS — that is the responsibility
of any observability tooling consuming `/proc/users/<u>/logs/`.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: SessionManifest
metadata:
  sessionId: sess-alice-001
  createdAt: 2026-03-15T07:30:00-05:00
  user: alice
  uid: 1001

spec:
  shell: /bin/sh
  tty: true                     # false for headless/API sessions
  workingDirectory: /users/alice/workspace

  agents:
    - pid: 57
      name: researcher
      role: primary
    - pid: 58
      name: writer
      role: subordinate

  quotaSnapshot:                # quota state at session open; live state tracked by kernel
    tokensUsed: 0
    tokensLimit: 500000
    agentsRunning: 0
    agentsLimit: 5

status:
  state: active                 # active | idle | closed
  lastActivityAt: 2026-03-15T07:41:12-05:00
  closedAt: null
  closedReason: null
```

-----

## Notes

- A session is created automatically on shell login or API connection and written to
  `/proc/users/<username>/sessions/` by the kernel.
- When the user logs out or the connection drops, the kernel sets `status.state: closed`
  and eventually removes the record from `/proc/`.
- `tty: false` sessions (headless/API) may not receive HIL prompts — the kernel queues
  HIL requests until a TTY session connects, or rejects them based on policy.
- `quotaSnapshot` is a point-in-time capture at session open. Live quota state is tracked
  by the kernel and readable at `/proc/users/<username>/status.yaml`.
- Closing a session sends `SIGSTOP` to all agents listed in `spec.agents`.

-----

## Related

- [Users](./users.md) — `quota.sessions` limits how many sessions a user may open concurrently
- [AgentStatus](./agent-status.md) — per-agent state for each PID listed in `spec.agents`
- [Signal](./signal.md) — closing a session sends `SIGSTOP` to all listed agents

-----

## Field Defaults

|Field                  |Default                      |Notes                                   |
|-----------------------|-----------------------------|----------------------------------------|
|`spec.shell`           |`/bin/sh`                    |                                        |
|`spec.tty`             |`true`                       |Set to `false` for headless/API sessions|
|`spec.workingDirectory`|`/users/<username>/workspace`|Derived from user record                |

System defaults at `/kernel/defaults/session-manifest.yaml`.
See [Defaults](./defaults.md).
