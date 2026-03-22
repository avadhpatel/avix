# Signal

← [Back to Schema Index](./README.md)

**Kind:** `Signal`  
**Direction:** Kernel ↔ Agent (both directions)

Signals are the primary control-plane events in Avix. They follow Unix signal semantics
but carry a structured YAML payload over the IPC channel rather than a bare integer.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Signal
metadata:
  from: kernel                  # kernel | <pid> | <username>
  to: 57                        # target pid, or "broadcast" for all agents
  sentAt: 2026-03-15T07:41:00-05:00
  signalId: sig-xyz999

spec:
  signal: SIGPAUSE              # see Signal Reference below
  reason: "Tool 'email' requires human approval before execution"
  payload:
    hilRequestId: hil-001       # present when signal is related to a HIL queue event
    pendingTool: email
    pendingArgs:
      to: user@example.com
      subject: Research summary ready
```

-----

## Signal Reference

|Signal       |Direction     |Meaning                                                                 |
|-------------|--------------|------------------------------------------------------------------------|
|`SIGSTART`   |Kernel → Agent|Agent has been spawned and should begin executing its goal              |
|`SIGPAUSE`   |Kernel ↔ Agent|Suspend execution; agent must not consume resources until `SIGRESUME`   |
|`SIGRESUME`  |Kernel → Agent|Resume after pause (e.g. human approved a tool call)                    |
|`SIGKILL`    |Kernel → Agent|Terminate immediately; no cleanup                                       |
|`SIGSTOP`    |Kernel → Agent|Graceful shutdown; agent should save state and exit cleanly             |
|`SIGSAVE`    |Kernel → Agent|Trigger an immediate snapshot                                           |
|`SIGPIPE`    |Kernel → Agent|Pipe partner has closed; agent should handle broken pipe                |
|`SIGUSR1`    |Agent → Kernel|Agent-defined event; payload is agent-specific                          |
|`SIGUSR2`    |Agent → Kernel|Secondary agent-defined event                                           |
|`SIGESCALATE`|Agent → Kernel|Agent requests human-in-the-loop escalation (quota, ethics, uncertainty)|

-----

## State Transitions

```
         SIGSTART
 pending ──────────► running
                        │
            SIGPAUSE ◄──┤──► SIGPAUSE (agent-initiated)
                        │
                     paused
                        │
           SIGRESUME ◄──┘
                        │
                     running
                        │
             SIGSTOP ◄──┤
                        │
                     stopped
                        │
              SIGKILL ◄─┘──► crashed (unexpected)
```

-----

## Related

- [AgentStatus](./agent-status.md) — `status.state` reflects the result of signals received
- [ResourceRequest](./resource-request.md) — `SIGPAUSE` is often sent while a HIL request is pending
- [Snapshot](./snapshot.md) — `SIGSAVE` and `SIGKILL`+crash both trigger snapshot creation
- [KernelConfig](./kernel-config.md) — `safety.hilOnEscalation` controls when `SIGPAUSE` is auto-issued
