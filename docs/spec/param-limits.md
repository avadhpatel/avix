# Limits

← [Back to Schema Index](./README.md)

**Kind:** `Limits`  
**Direction:** Runtime (kernel-owned and dynamic)

Limits files define the bounds within which defaults and agent manifest values must fall.
They are **kernel-owned** — the system limits are initialised at boot from compiled-in
values and may be updated by the kernel at runtime (e.g. under memory pressure, model
availability changes, or policy updates).

User and crew limits may be authored by operators with the right permissions, but they
can only **narrow** the system limits — never widen them.

-----

## Locations

|Layer          |Path                                |Written by                           |Editable by       |
|---------------|------------------------------------|-------------------------------------|------------------|
|System         |`/kernel/limits/<kind>.yaml`        |Kernel (dynamic)                     |Nobody (read-only)|
|System (tools) |`/kernel/limits/tools/<tool>.yaml`  |Kernel (dynamic)                     |Nobody (read-only)|
|System (models)|`/kernel/limits/models/<model>.yaml`|Kernel (dynamic)                     |Nobody (read-only)|
|User           |`/users/<username>/limits.yaml`     |User (with `limits:write` permission)|User              |
|Service        |`/services/<svcname>/limits.yaml`   |Service operator                     |Service operator  |
|Crew           |`/crews/<crew-name>/limits.yaml`    |Crew admin                           |Crew admin        |

-----

## Constraint Types

|Type      |Description                                      |Example                              |
|----------|-------------------------------------------------|-------------------------------------|
|`range`   |Numeric min/max (inclusive)                      |`min: 1, max: 20`                    |
|`enum`    |Value must be one of a fixed set                 |`values: [lru, lru_salience, manual]`|
|`set`     |List membership — allowed or denied items        |`allowed: [web, read, write]`        |
|`bool`    |Field may be locked to a specific value          |`value: false` (prevents enabling)   |
|`readonly`|Field cannot be overridden at this layer or above|`readonly: true`                     |

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Limits
metadata:
  target: agent-manifest        # the kind these limits apply to
  layer: system                 # system | user | crew
  owner: null                   # null for system; username or crew-name otherwise
  updatedAt: 2026-03-15T07:38:00-05:00
  updatedBy: kernel             # kernel | username
  reason: boot                  # boot | memory-pressure | model-unavailable | policy-update | manual

limits:
  entrypoint:
    modelPreference:
      type: enum
      values: [claude-sonnet-4, claude-haiku-4]   # opus not available at this tier
    minContextTokens:
      type: range
      min: 1000
      max: 32000
    maxToolChain:
      type: range
      min: 1
      max: 10

  tools:
    required:  # tools required; spawn fails if denied
      type: set
      allowed: [web, read, write, code_exec, email, db, pipe, spawn, snapshot]
    optional:
      type: set
      allowed: [web, read, write, code_exec, email, db, pipe, spawn, snapshot]

  memory:
    workingContext:
      type: enum
      values: [fixed, dynamic]
    semanticStoreAccess:
      type: enum
      values: [none, read-only]   # read-write not permitted at this tier

  snapshot:
    enabled:
      type: bool
      value: null                 # null = not locked; agent can set freely
    autoSnapshotIntervalSec:
      type: range
      min: 0
      max: 3600

  defaults:
    environment:
      temperature:
        type: range
        min: 0.0
        max: 1.0
      timeoutSec:
        type: range
        min: 30
        max: 600
```

### Example — user-level limits

A user can publish their own limits to constrain agents they spawn, within the system limits:

```yaml
apiVersion: avix/v1
kind: Limits
metadata:
  target: agent-manifest
  layer: user
  owner: alice
  updatedAt: 2026-03-15T09:00:00-05:00
  updatedBy: alice
  reason: manual

limits:
  entrypoint:
    maxToolChain:
      type: range
      min: 1
      max: 6               # alice caps her agents lower than system allows

  defaults:
    environment:
      timeoutSec:
        type: range
        min: 30
        max: 300            # alice's agents may not run longer than 5 minutes
```

### Example — system tool limits (dynamic)

```yaml
apiVersion: avix/v1
kind: Limits
metadata:
  target: tool
  layer: system
  owner: null
  updatedAt: 2026-03-15T10:14:00-05:00
  updatedBy: kernel
  reason: memory-pressure     # kernel tightened this at runtime

limits:
  web:
    maxResultsPerQuery:
      type: range
      min: 1
      max: 5                  # reduced from 10 at boot due to memory pressure
    timeoutSec:
      type: range
      min: 5
      max: 20

  email:
    maxRecipientsPerCall:
      type: range
      min: 1
      max: 3
```

-----

## Conflict Resolution Between Crews

When a user belongs to multiple crews with conflicting limits, the kernel applies the
**tightest constraint** across all applicable crew limits. For `range` types, this means
the lowest `max` and highest `min`. For `set` and `enum` types, this means the
intersection of allowed values.

Example: if `researchers` allows `maxToolChain.max: 10` and `writers` allows
`maxToolChain.max: 5`, the effective limit for a user in both crews is `max: 5`.

Use `avix resolve --explain` to see how limits were derived across crews.

-----

## Notes

- System limits at `/kernel/limits/` are **read-only from outside the kernel**. Attempts
  to write to these paths are rejected with `EPERM`.
- When the kernel updates a limits file at runtime, it emits a `SIGUSR1` to all affected
  running agents so they can re-read their resolved config if needed.
- A limits file with no entry for a field means that field is unconstrained at that layer
  — the next lower layer’s limit applies.
- The `readonly: true` constraint type locks a field from being overridden at any layer
  above the one that sets it, including by agent manifests.

-----

## Related

- [Defaults](./defaults.md) — values must fall within these limits
- [Resolved](./resolved.md) — merged output showing effective limits and values
- [Signal](./signal.md) — kernel emits `SIGUSR1` to agents when limits change at runtime
- [README — Resolution Order](./README.md#resolution-order) — full precedence chain
