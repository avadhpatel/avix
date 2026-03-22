# Crews

← [Back to Schema Index](./README.md)

**Kind:** `Crews`  
**Location:** `/etc/avix/crews.yaml`  
**Direction:** Config (static)

-----

## Crews vs. Unix Groups

Crews serve the same primary purpose as Unix groups — ACL membership that controls
file and resource access — and use the same permission-bit model (`owner/crew/world`).

The key difference is **membership scope**:

|Aspect             |Unix `group`          |Avix `crew`                                                             |
|-------------------|----------------------|------------------------------------------------------------------------|
|Members            |Users only            |Users **and** agents (by PID or agent name)                             |
|`sharedPaths`      |Via `chgrp` externally|First-class field in the spec                                           |
|Tool access control|No                    |`allowedTools` / `deniedTools` define the base tool set for the crew    |
|Inter-member trust |File ACL only         |Agents in the same crew may pipe to each other without a ResourceRequest|
|Collective quota   |No                    |Planned — crew-level token budget (future `v2`)                         |

Because agents can be crew members, a crew can represent a **collaborative unit** — e.g.
a `researchers` crew whose members are both the human operator `alice` and any
`researcher`-template agents she spawns. Those agents inherit crew-level access to
`/crews/researchers/shared/research/` and the crew’s permitted tool set without needing
individual grants.

When this distinction does not matter (purely user-grouping for ACL), a crew behaves
exactly like a Unix group. The naming divergence is intentional: `crew` signals that
agent membership is a first-class concept in Avix.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Crews
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  crews:
    - name: all
      cid: 0
      description: Every user; world-readable access baseline
      members: ["*"]            # wildcard = all users and all agents

    - name: kernel
      cid: 1
      description: Kernel and system-level agents only
      members: [root]

    - name: researchers
      cid: 1001
      description: Human researchers and any researcher-template agents they spawn
      members:
        - user:alice                        # human operator
        - agent:researcher                  # any running instance of the researcher template
      agentInheritance: spawn               # spawn | explicit | none
      allowedTools:                         # base tool set for agents spawned by crew members
        - web_search
        - web_fetch
        - file_read
        - file_write
      deniedTools:                          # explicitly blocked even if user ACL would allow
        - bash
        - send_email
      sharedPaths:
        - /crews/researchers/shared/research/
      pipePolicy: allow-intra-crew          # allow-intra-crew | require-request | deny

    - name: writers
      cid: 1002
      description: Content generation agents and their owning users
      members:
        - user:alice
        - agent:writer
      agentInheritance: spawn
      allowedTools:
        - file_read
        - file_write
        - web_search
      deniedTools:
        - bash
        - python
      sharedPaths:
        - /crews/writers/shared/drafts/
      pipePolicy: allow-intra-crew

    - name: automation
      cid: 2001
      description: Headless service accounts and scheduled pipeline agents
      members:
        - user:svc-pipeline
        - agent:pipeline-ingest
        - agent:memory-gc
      agentInheritance: none                # automation agents must be added explicitly
      allowedTools:
        - web_search
        - web_fetch
        - file_read
        - file_write
        - python
        - http_request
      deniedTools:
        - bash
        - send_email
      sharedPaths:
        - /crews/automation/shared/pipeline/
      pipePolicy: require-request
```

-----

## Field Reference

|Field             |Values                                         |Description                                               |
|------------------|-----------------------------------------------|----------------------------------------------------------|
|`cid`             |integer                                        |Crew ID; 0–999 reserved                                   |
|`members[]`       |`user:<n>`, `agent:<template>`, `"*"`          |Typed member list; `"*"` = wildcard                       |
|`agentInheritance`|`spawn` | `explicit` | `none`                  |Whether agents auto-join when spawned by a member user    |
|`allowedTools[]`  |tool names                                     |Base permitted tool set for agents spawned by crew members|
|`deniedTools[]`   |tool names                                     |Tools explicitly blocked; overrides user `additionalTools`|
|`sharedPaths[]`   |paths under `/crews/<n>/shared/`               |Paths all crew members have read-write access to          |
|`pipePolicy`      |`allow-intra-crew` | `require-request` | `deny`|Whether intra-crew pipes bypass the ResourceRequest cycle |

-----

## Related

- [Users](./users.md) — users reference crew names in `users[].crews`; `additionalTools` and `deniedTools` layer on top of crew tool sets
- [AgentManifest](./agent-manifest.md) — `spec.tools` is intersected with the crew’s `allowedTools` at spawn
- [CapabilityToken](./capability-token.md) — token reflects the resolved tool grant after crew + user intersection
- [Pipe](./pipe.md) — `pipePolicy: allow-intra-crew` bypasses [ResourceRequest](./resource-request.md)
