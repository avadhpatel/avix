# Users

← [Back to Schema Index](./README.md)

**Kind:** `Users`  
**Location:** `/etc/avix/users.yaml`  
**Direction:** Config (static)

Defines human operators and service accounts. UIDs below 1000 are reserved for kernel
and system agents.

> **Reserved UIDs:** `0` = root, `1`–`99` = kernel internals, `100`–`999` = system agents.  
> Each user gets a portable workspace tree at `/users/<username>/` — this entire tree
> can be mounted, backed up, or migrated to another Avix instance independently.  
> Service accounts live under `/services/<svcname>/` and use `shell: nologin`.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Users
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  users:
    - username: root
      uid: 0
      home: /root
      shell: /bin/sh
      crews: [all, kernel]
      tools: [all]              # root has access to all tools
      quota:
        tokens: unlimited
        agents: unlimited
        sessions: unlimited

    - username: alice
      uid: 1001
      workspace: /users/alice/workspace
      shell: /bin/sh
      crews: [researchers, writers]
      additionalTools:           # tools granted on top of crew allowedTools
        - python
      deniedTools: []            # tools explicitly blocked even if crew allows them
      quota:
        tokens: 500000           # rolling 24h window
        agents: 5                # max concurrently running agents
        sessions: 4              # max concurrent interactive sessions

    - username: svc-pipeline
      uid: 2001
      workspace: /services/svc-pipeline/workspace
      shell: nologin             # no interactive shell; automation only
      crews: [automation]
      additionalTools: []
      deniedTools: []
      quota:
        tokens: 1000000
        agents: 10
        sessions: 1
```

-----

## Related

- [Crews](./crews.md) — crew definitions referenced by `users[].crews`
- [CapabilityToken](./capability-token.md) — tokens reflect tool grants derived from crew allowedTools + user additionalTools
- [SessionManifest](./session-manifest.md) — sessions are bounded by `users[].quota.sessions`
