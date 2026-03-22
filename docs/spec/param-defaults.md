# Defaults

← [Back to Schema Index](./README.md)

**Kind:** `Defaults`  
**Direction:** Config (layered)

Defaults files provide fallback values for any field not explicitly set in an agent
manifest or runtime request. They exist at multiple layers of the resolution hierarchy
and are merged by the kernel at spawn time.

Defaults files use a **nested structure** that mirrors the `spec` block of the target
kind — so a defaults file for `agent-manifest` looks like a partial `AgentManifest.spec`.
This means the file is both human-readable and parseable in a single pass.

-----

## Locations

|Layer          |Path                                  |Written by      |Editable by                     |
|---------------|--------------------------------------|----------------|--------------------------------|
|System         |`/kernel/defaults/<kind>.yaml`        |Build time      |Nobody (read-only)              |
|System (tools) |`/kernel/defaults/tools/<tool>.yaml`  |Build time      |Nobody (read-only)              |
|System (models)|`/kernel/defaults/models/<model>.yaml`|Build time      |Nobody (read-only)              |
|User           |`/users/<username>/defaults.yaml`     |User            |User (within limits)            |
|Service        |`/services/<svcname>/defaults.yaml`   |Service operator|Service operator (within limits)|
|Crew           |`/crews/<crew-name>/defaults.yaml`    |Crew admin      |Crew admin (within limits)      |

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Defaults
metadata:
  target: agent-manifest        # the kind these defaults apply to
  layer: system                 # system | user | crew
  owner: null                   # null for system; username or crew-name otherwise
  updatedAt: 2026-03-15T07:38:00-05:00

defaults:
  entrypoint:
    type: llm-loop
    modelPreference: claude-sonnet-4
    minContextTokens: 8000
    maxToolChain: 5

  memory:
    workingContext: dynamic
    episodicPersistence: false
    semanticStoreAccess: none

  snapshot:
    enabled: false
    autoSnapshotIntervalSec: 0
    restoreOnCrash: false

  defaults:
    environment:
      temperature: 0.7
      timeoutSec: 300

  permissionsHint:
    owner: rw
    crew: r
    world: r--
```

### Example — user-level defaults override

A user can raise their personal defaults within their permitted limits:

```yaml
apiVersion: avix/v1
kind: Defaults
metadata:
  target: agent-manifest
  layer: user
  owner: alice
  updatedAt: 2026-03-15T09:00:00-05:00
  # stored at /users/alice/defaults.yaml under the agent-manifest key

defaults:
  entrypoint:
    maxToolChain: 8             # alice prefers more tool calls by default
    modelPreference: claude-opus-4

  snapshot:
    enabled: true               # alice wants snapshots on by default
    autoSnapshotIntervalSec: 300
    restoreOnCrash: true
```

### Example — system tool defaults

```yaml
apiVersion: avix/v1
kind: Defaults
metadata:
  target: tool
  layer: system
  owner: null
  updatedAt: 2026-03-15T07:38:00-05:00

defaults:
  web:
    timeoutSec: 30
    maxResultsPerQuery: 10
    allowedSchemes: [https]

  email:
    timeoutSec: 15
    maxRecipientsPerCall: 5
```

-----

## Notes

- The `defaults:` top-level key (not `spec:`) signals that this file is a defaults
  declaration, not a full manifest.
- Fields omitted from a defaults file simply have no default at that layer — the next
  lower layer’s value applies.
- User and crew defaults are validated against the effective [Limits](./limits.md) at
  write time. The kernel rejects a defaults file that sets a value outside permitted limits.

-----

## Related

- [Limits](./limits.md) — bounds within which defaults and manifest values must fall
- [Resolved](./resolved.md) — the merged output after all layers are applied
- [README — Resolution Order](./README.md#resolution-order) — full precedence chain
