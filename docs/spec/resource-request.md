# ResourceRequest

← [Back to Schema Index](./README.md)

**Kind:** `ResourceRequest`  
**Direction:** Agent → Kernel (IPC syscall)

Sent when an agent needs additional resources, tools, or pipe access that were not
granted at spawn time. The agent must present its [CapabilityToken](./capability-token.md)
on every request.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: ResourceRequest
metadata:
  agentPid: 57
  requestId: req-abc123
  timestamp: 2026-03-15T07:38:22-05:00
  capabilityToken: sha256:tokenSig789...  # must match CapabilityToken.spec.signature

spec:
  requests:
    - resource: context_tokens
      amount: 50000
      reason: Need a longer research thread for multi-document synthesis

    - resource: tool
      name: email
      reason: Notify user when summary is complete
      urgency: low              # low | normal | high — informs HIL queue priority

    - resource: pipe
      targetPid: 58
      direction: out            # in | out | bidirectional
      bufferTokens: 16384
      reason: Stream intermediate results to writer agent

    - resource: token_renewal
      reason: Current token expires in 5 minutes
```

-----

## Resource Types

|`resource`      |Required fields                         |Description                                  |
|----------------|----------------------------------------|---------------------------------------------|
|`context_tokens`|`amount`                                |Request additional context window tokens     |
|`tool`          |`name`, `urgency`                       |Request access to a tool not in current token|
|`pipe`          |`targetPid`, `direction`, `bufferTokens`|Request a pipe to another agent              |
|`token_renewal` |—                                       |Renew expiring CapabilityToken               |

-----

## Related

- [ResourceResponse](./resource-response.md) — kernel’s reply to this request
- [CapabilityToken](./capability-token.md) — must be presented on every request
- [Signal](./signal.md) — kernel may send `SIGPAUSE` while a HIL-gated request is pending
- [Crews](./crews.md) — `pipePolicy: allow-intra-crew` bypasses the pipe ResourceRequest cycle

-----

## Field Defaults

|Field                         |Default |Notes                                   |
|------------------------------|--------|----------------------------------------|
|`spec.requests[].urgency`     |`normal`|Applies to `tool` resource requests only|
|`spec.requests[].direction`   |`out`   |Applies to `pipe` resource requests only|
|`spec.requests[].bufferTokens`|`8192`  |Applies to `pipe` resource requests only|

See [Defaults](./defaults.md).
