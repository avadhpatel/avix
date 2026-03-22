# Resolved

← [Back to Schema Index](./README.md)

**Kind:** `Resolved`  
**Direction:** Runtime (kernel-derived, read-only)

Resolved files are the kernel’s final word on what values an agent actually runs with.
They are produced at spawn time by merging all defaults layers, clamping against all
limits layers, and applying any manifest overrides. They are never authored — only read.

Two resolved files exist:

- **Per-agent** at `/proc/<pid>/resolved.yaml` — written at spawn, reflects the exact
  config this agent instance is running with.
- **Per-user preview** at `/kernel/resolved/<username>/<kind>.yaml` — written whenever
  the resolution inputs change (limits update, user defaults change). Shows what a
  hypothetical agent spawn would get, before spawning.

-----

## Locations

|Path                                           |When written       |Purpose                                |
|-----------------------------------------------|-------------------|---------------------------------------|
|`/proc/<pid>/resolved.yaml`                    |At agent spawn     |Authoritative per-agent config         |
|`/proc/users/<username>/resolved/<kind>.yaml`  |On any input change|Pre-spawn preview for users/admins     |
|`/proc/services/<svcname>/resolved/<kind>.yaml`|On any input change|Pre-spawn preview for service operators|

Both paths are **read-only** for all users and agents.

-----

## Schema

```yaml
apiVersion: avix/v1
kind: Resolved
metadata:
  target: agent-manifest
  resolvedAt: 2026-03-15T07:38:00-05:00
  resolvedFor:
    username: alice
    pid: 57                     # present in /proc/<pid>/resolved.yaml; null in preview
  crews: [researchers, writers]

resolved:
  entrypoint:
    type: llm-loop
    modelPreference: claude-sonnet-4
    minContextTokens: 16000
    maxToolChain: 6

  tools:
    required: [web, read]
    optional: [code_exec]       # email was requested but denied by alice's limits

  memory:
    workingContext: dynamic
    episodicPersistence: true
    semanticStoreAccess: read-only

  snapshot:
    enabled: true
    autoSnapshotIntervalSec: 300
    restoreOnCrash: true

  defaults:
    environment:
      temperature: 0.7
      timeoutSec: 300

  permissionsHint:
    owner: rw
    crew: r
    world: r--

# Annotation block — shows provenance of every resolved value
# Present in preview files and when resolved with --explain flag
annotations:
  entrypoint.maxToolChain:
    value: 6
    source: user-defaults       # layer that provided the winning value
    path: /users/alice/defaults.yaml
    clampedFrom: 8              # manifest requested 8; clamped to crew limit of 6
    clampedBy: /crews/writers/limits.yaml

  entrypoint.modelPreference:
    value: claude-sonnet-4
    source: user-defaults
    path: /users/alice/defaults.yaml

  memory.semanticStoreAccess:
    value: read-only
    source: system-limits       # system limit blocked read-write; fell back to default
    path: /kernel/limits/agent-manifest.yaml

  snapshot.enabled:
    value: true
    source: user-defaults
    path: /users/alice/defaults.yaml

  tools.optional:
    value: [code_exec]
    source: user-limits
    path: /users/alice/limits.yaml
    note: email removed — not in alice's allowed capability set
```

-----

## Annotation Sources

|`source` value   |Meaning                                                          |
|-----------------|-----------------------------------------------------------------|
|`system-defaults`|Value came from `/kernel/defaults/`                              |
|`system-limits`  |Value was constrained or set by `/kernel/limits/`                |
|`user-defaults`  |Value came from user’s defaults file                             |
|`user-limits`    |Value was constrained by user’s limits file                      |
|`crew-defaults`  |Value came from a crew defaults file                             |
|`crew-limits`    |Value was constrained by a crew limits file (tightest crew wins) |
|`manifest`       |Value was set directly in the AgentManifest and passed all limits|

-----

## CLI Triage Tool

Admins and users can inspect the full resolution trace without spawning an agent:

```sh
# Show resolved config for alice spawning a researcher agent
avix resolve agent-manifest --user alice --agent researcher

# Show with full annotation (provenance of every field)
avix resolve agent-manifest --user alice --agent researcher --explain

# Show what limits are in effect for alice across all crews
avix resolve agent-manifest --user alice --explain --limits-only

# Simulate what would happen if alice joined the automation crew
avix resolve agent-manifest --user alice --crew automation --dry-run
```

Output is a `Resolved` YAML document written to stdout.

-----

## Notes

- `/proc/<pid>/resolved.yaml` is immutable for the lifetime of the agent — it captures
  the exact config at spawn. If limits change at runtime (kernel emits `SIGUSR1`), the
  agent’s resolved file does **not** change; only new spawns are affected.
- The `annotations` block is omitted from `/proc/<pid>/resolved.yaml` by default to
  keep the file compact. Pass `--explain` to `avix inspect <pid>` to get the annotated
  version.
- If the kernel cannot produce a valid resolved config (e.g. manifest requests a
  capability outside all permitted sets), spawn is rejected and the kernel writes a
  `ResolveError` to `/proc/spawn-errors/<request-id>.yaml`.

-----

## Related

- [Defaults](./defaults.md) — input layer 1: fallback values
- [Limits](./limits.md) — input layer 2: bounds and constraints
- [AgentManifest](./agent-manifest.md) — input layer 3: agent author’s requested values
- [AgentStatus](./agent-status.md) — runtime state; resolved.yaml is the config it runs under
- [README — Resolution Order](./README.md#resolution-order) — full precedence chain
