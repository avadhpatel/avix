# CapabilityToken

← [Back to Schema Index](./README.md)

**Kind:** `CapabilityToken`  
**Location:** Issued by kernel at spawn; passed to agent as `AVIX_CAP_TOKEN` env var  
**Direction:** Runtime (issued)

Issued by the kernel on agent spawn. Agents present this token on every
[ResourceRequest](./resource-request.md). The kernel validates the signature and expiry
before granting any resource.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: CapabilityToken
metadata:
  issuedAt: 2026-03-15T07:38:00-05:00
  expiresAt: 2026-03-15T08:38:00-05:00  # tokens expire; agent must request renewal
  issuedTo:
    pid: 57
    agentName: researcher
    spawnedBy: alice

spec:
  tools:
    granted:
      - web_search
      - web_fetch
      - file_read
    # send_email was optional in manifest but denied by crew limits — not listed

  constraints:
    maxTokensPerTurn: 8000
    maxToolChainLength: 8
    allowPipeTargets: [58]         # PIDs this agent is allowed to pipe to

  signature: sha256:tokenSig789... # HMAC-signed by kernel; agents must not modify
```

-----

## Notes

- `spec.tools.granted` reflects only the tools actually granted at spawn — optional tools
  denied by crew or user limits are absent.
- The `signature` field is HMAC-signed by the kernel. Any modification invalidates the
  token and causes the kernel to reject subsequent requests from that agent.
- When a token nears expiry, the agent should send a `ResourceRequest` with
  `resource: token_renewal` before the deadline.
- Agents must treat the token as opaque — do not parse or rely on internal structure
  beyond what is defined here.

-----

## Related

- [AgentManifest](./agent-manifest.md) — `spec.tools` declares what the agent requests
- [Users](./users.md) — user ACL is the upper bound on what tools can be granted
- [Crews](./crews.md) — crew limits define the permitted tool set for members
- [ResourceRequest](./resource-request.md) — token must be presented on every request
