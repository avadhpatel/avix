# Avix YAML Schema References (v1)

All configuration, manifests, requests, and responses in Avix use YAML with a consistent
structure inspired by Kubernetes-style manifests.

-----

## General Conventions

|Field       |Rule                                                                                 |
|------------|-------------------------------------------------------------------------------------|
|`apiVersion`|Always `avix/v1`; bump to `avix/v2` on breaking changes                              |
|`kind`      |PascalCase string identifying the resource type                                      |
|`metadata`  |Optional free-form provenance fields (name, version, labels, annotations, timestamps)|
|`spec`      |**Required** — desired state / configuration                                         |
|`status`    |Optional — runtime-observed state; written by kernel, **read-only** for agents/users |

- All timestamps use ISO 8601 with timezone: `2026-03-15T07:38:00-05:00`
- All durations use seconds unless noted: `timeoutSec: 300`
- Comments (`#`) are encouraged everywhere
- Unknown fields are rejected by the kernel validator unless `spec.strict: false`

-----

## Schema Index

|# |Kind            |Location                               |Direction         |
|--|----------------|---------------------------------------|------------------|
|1 |AgentManifest   |`/bin/<agent>/manifest.yaml`           |Config (static)   |
|2 |AgentStatus     |Written by kernel at runtime           |Status (dynamic)  |
|3 |Users           |`/etc/avix/users.yaml`                 |Config (static)   |
|4 |Crews           |`/etc/avix/crews.yaml`                 |Config (static)   |
|5 |KernelConfig    |`/etc/avix/kernel.yaml`                |Config (static)   |
|6 |CapabilityToken |Issued by kernel at spawn time         |Runtime (issued)  |
|7 |ResourceRequest |Agent → Kernel (IPC syscall)           |Runtime (request) |
|8 |ResourceResponse|Kernel → Agent (IPC reply)             |Runtime (response)|
|9 |Signal          |Kernel ↔ Agent (IPC event)             |Runtime (event)   |
|10|Pipe            |`/proc/<pid>/pipes/<id>.yaml` (runtime)|Runtime (channel) |
|11|Crontab         |`/etc/avix/crontab.yaml`               |Config (static)   |
|12|Snapshot        |`/var/avix/snapshots/<name>.yaml`      |Persistence       |
|13|SessionManifest |`/var/avix/sessions/<sid>.yaml`        |Runtime (session) |

-----

## 1. AgentManifest

**Location:** `/bin/<agent>/manifest.yaml` or `/installed/<agent>/manifest.yaml`

Defines an agent’s static identity, capabilities, and default behaviour. Loaded once at
install time and re-read on spawn. Never mutated at runtime.

```yaml
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: researcher
  version: 1.3.0
  description: General-purpose web & document researcher
  author: kernel-team
  createdAt: 2026-03-10T14:22:00Z
  signature: sha256:abc123def456...   # optional — verified by kernel on install

spec:
  entrypoint:
    type: llm-loop                    # llm-loop | custom-script (future)
    modelPreference: claude-sonnet-4  # overridden by KernelConfig.models.default if absent
    minContextTokens: 32000
    maxToolChain: 8                   # max sequential tool calls per turn

  capabilities:
    required:                         # spawn is rejected if kernel cannot grant these
      - web
      - read
    optional:                         # requested at spawn; silently skipped if unavailable
      - code_exec
      - email

  memory:
    workingContext: dynamic           # fixed | dynamic
    episodicPersistence: true         # whether episodic events are written to /memory/
    semanticStoreAccess: read-only    # none | read-only | read-write

  snapshot:
    enabled: true                     # whether this agent supports snapshot/restore
    autoSnapshotIntervalSec: 600      # 0 = disabled; kernel triggers periodic snapshots
    restoreOnCrash: true              # kernel auto-restores last snapshot on SIGKILL+crash

  defaults:
    goalTemplate: |
      Research and summarize: {{topic}}.
      Include sources and a confidence score.
      Format output as markdown with sections.
    environment:
      temperature: 0.7
      timeoutSec: 300

  permissionsHint:                    # advisory; kernel enforces actual ACL from users.yaml
    owner: rw
    crew: r
    world: r--
```

-----

## 2. AgentStatus

**Written by kernel** into `/proc/<pid>/status.yaml` at runtime. Agents and users may
read this; only the kernel may write it.

```yaml
apiVersion: avix/v1
kind: AgentStatus
metadata:
  name: researcher
  pid: 57
  spawnedAt: 2026-03-15T07:38:00-05:00
  spawnedBy: alice          # username or pid of parent agent

status:
  state: running            # pending | running | paused | waiting | stopped | crashed
  goal: "Research quantum computing breakthroughs 2025"
  contextUsed: 64000        # tokens currently in working context
  contextLimit: 200000
  toolCallsThisTurn: 3
  lastActivityAt: 2026-03-15T07:41:12-05:00
  waitingOn: null           # null | human-approval | pipe-read | pipe-write | signal

  capabilities:
    granted:
      - web
      - read
    denied:
      - email               # denied at spawn; would require human approval

  pipes:
    - id: pipe-001
      targetPid: 58
      direction: out
      state: open

  signals:
    lastReceived: null
    pendingCount: 0

  metrics:
    tokensConsumed: 14200   # this session
    toolCallsTotal: 11
    wallTimeSec: 192
```

-----

## 3. Users

**Location:** `/etc/avix/users.yaml`

Defines human operators and service accounts. UIDs below 1000 are reserved for kernel
and system agents.

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
      capabilities: [all]
      quota:
        tokens: unlimited
        agents: unlimited
        sessions: unlimited

    - username: alice
      uid: 1001
      home: /home/alice
      shell: /bin/sh
      crews: [researchers, writers]
      capabilities: [web, read, write]
      quota:
        tokens: 500000         # rolling 24h window
        agents: 5              # max concurrently running agents
        sessions: 4            # max concurrent interactive sessions

    - username: svc-pipeline
      uid: 2001
      home: /srv/pipeline
      shell: nologin           # no interactive shell; automation only
      crews: [automation]
      capabilities: [web, file_io, db]
      quota:
        tokens: 1000000
        agents: 10
        sessions: 1
```

> **Reserved UIDs:** `0` = root, `1`–`99` = kernel internals, `100`–`999` = system agents.
> Service accounts use `shell: nologin` to prevent interactive sessions.

-----

## 4. Crews

**Location:** `/etc/avix/crews.yaml`

### Crews vs. Unix Groups

Crews serve the same primary purpose as Unix groups — ACL membership that controls
file and resource access — and use the same permission-bit model (`owner/crew/world`).

The key difference is **membership scope**:

|Aspect            |Unix `group`          |Avix `crew`                                                                      |
|------------------|----------------------|---------------------------------------------------------------------------------|
|Members           |Users only            |Users **and** agents (by PID or agent name)                                      |
|`sharedPaths`     |Via `chgrp` externally|First-class field in the spec                                                    |
|Inter-member trust|File ACL only         |Agents in the same crew may pipe to each other without a separate ResourceRequest|
|Collective quota  |No                    |Planned — crew-level token budget (future `v2`)                                  |

Because agents can be crew members, a crew can represent a **collaborative unit** — e.g.
a `researchers` crew whose members are both the human operator `alice` and any
`researcher`-template agents she spawns. Those agents inherit crew-level access to
`/shared/research/` without needing individual grants.

When this distinction does not matter (purely user-grouping for ACL), a crew behaves
exactly like a Unix group. The naming divergence is intentional: `crew` signals that
agent membership is a first-class concept in Avix.

```yaml
apiVersion: avix/v1
kind: Crews
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  crews:
    - name: all
      gid: 0
      description: Every user; world-readable access baseline
      members: ["*"]            # wildcard = all users and all agents

    - name: kernel
      gid: 1
      description: Kernel and system-level agents only
      members: [root]

    - name: researchers
      gid: 1001
      description: Human researchers and any researcher-template agents they spawn
      members:
        - user:alice                        # human operator
        - agent:researcher                  # any running instance of the researcher template
      agentInheritance: spawn               # agents inherit crew if spawned by a member user
                                            # spawn | explicit | none
      sharedPaths:
        - /shared/research/                 # crew members share read-write access
      pipePolicy: allow-intra-crew          # members may pipe to each other without ResourceRequest
                                            # allow-intra-crew | require-request | deny

    - name: writers
      gid: 1002
      description: Content generation agents and their owning users
      members:
        - user:alice
        - agent:writer
      agentInheritance: spawn
      sharedPaths:
        - /shared/drafts/
      pipePolicy: allow-intra-crew

    - name: automation
      gid: 2001
      description: Headless service accounts and scheduled pipeline agents
      members:
        - user:svc-pipeline
        - agent:pipeline-ingest
        - agent:memory-gc
      agentInheritance: none                # automation agents must be added explicitly
      sharedPaths:
        - /shared/pipeline/
      pipePolicy: require-request
```

-----

## 5. KernelConfig

**Location:** `/etc/avix/kernel.yaml`

Master configuration for the Avix kernel. Reload with `avix reload` (no restart required
for most fields; `ipc` and `models.kernel` require restart).

```yaml
apiVersion: avix/v1
kind: KernelConfig
metadata:
  lastUpdated: 2026-03-15T07:38:00-05:00

spec:
  scheduler:
    algorithm: priority_deadline   # priority_deadline | round_robin | fifo
    tickMs: 100
    preemption: true               # allow kernel to pause lower-priority agents
    maxConcurrentAgents: 50

  memory:
    defaultContextLimit: 200000    # tokens; per-agent unless overridden in manifest
    evictionPolicy: lru_salience   # lru | lru_salience | manual
    maxEpisodicRetentionDays: 30
    sharedMemoryPath: /shared/

  ipc:
    transport: unix-socket         # unix-socket | grpc (future)
    socketPath: /var/run/avix/kernel.sock
    maxMessageBytes: 65536
    timeoutMs: 5000

  safety:
    policyEngine: enabled
    hilOnEscalation: true          # pause and surface to human when escalation detected
    maxToolChainLength: 10
    blockedToolChains:
      - pattern: "email + code_exec"
        reason: high risk of data exfiltration
      - pattern: "db + web"
        reason: potential data leak to external endpoints

  models:
    default: claude-sonnet-4       # used for agent spawns lacking a modelPreference
    kernel: claude-opus-4          # used for kernel-internal reasoning (policy, routing)
    fallback: claude-haiku-4       # used when quota is near limit or primary unavailable
    temperature: 0.7

  observability:
    logLevel: info                 # debug | info | warn | error
    logPath: /var/log/avix/kernel.log
    metricsEnabled: true
    metricsPath: /var/log/avix/metrics/
    traceEnabled: false            # structured trace per agent turn; high storage cost
```

-----

## 6. CapabilityToken

**Issued by kernel** on agent spawn; passed to the agent as an environment variable
(`AVIX_CAP_TOKEN`). Agents present this token on every ResourceRequest. The kernel
validates signature + expiry before granting any resource.

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
  capabilities:
    - web
    - read
  # note: 'email' was optional in the manifest but denied at spawn — not listed here

  constraints:
    maxTokensPerTurn: 8000
    maxToolChainLength: 8
    allowPipeTargets: [58]         # PIDs this agent is allowed to pipe to

  signature: sha256:tokenSig789... # HMAC-signed by kernel; agents must not modify
```

-----

## 7. ResourceRequest

**Direction:** Agent → Kernel (IPC / syscall payload)

Sent when an agent needs additional resources, tools, or pipe access that were not
granted at spawn time.

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
```

-----

## 8. ResourceResponse

**Direction:** Kernel → Agent (IPC reply)

Kernel’s authoritative reply to a ResourceRequest. Agents must check `granted` before
using any resource.

```yaml
apiVersion: avix/v1
kind: ResourceResponse
metadata:
  requestId: req-abc123
  respondedAt: 2026-03-15T07:38:25-05:00

spec:
  grants:
    - resource: context_tokens
      granted: true
      amount: 50000
      newTotal: 114000
      expiresAt: null             # null = for lifetime of session

    - resource: tool
      name: email
      granted: false
      reason: Requires human-in-the-loop approval
      suggestion: Send SIGPAUSE and present request to user via /proc/57/hil-queue

    - resource: pipe
      targetPid: 58
      granted: true
      pipeId: pipe-001
      direction: out
      bufferTokens: 16384
```

-----

## 9. Signal

**Direction:** Kernel ↔ Agent (both directions possible)

Signals are the primary control-plane events in Avix. They follow Unix signal semantics
but carry a structured YAML payload over the IPC channel.

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

### Signal Reference

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

## 10. Pipe

**Location:** `/proc/<pid>/pipes/<pipe-id>.yaml` (written by kernel at runtime)

A unidirectional or bidirectional token-stream channel between two agents. Created via
ResourceRequest and destroyed when either agent exits.

```yaml
apiVersion: avix/v1
kind: Pipe
metadata:
  pipeId: pipe-001
  createdAt: 2026-03-15T07:38:25-05:00
  createdBy: kernel

spec:
  sourcePid: 57
  targetPid: 58
  direction: out               # out (57→58) | in (58→57) | bidirectional
  bufferTokens: 16384
  backpressure: block          # block | drop | error — behaviour when buffer is full

  encoding: text               # text | json | yaml — payload format convention

status:
  state: open                  # open | closed | error
  tokensSent: 4200
  tokensAcknowledged: 4200
  closedAt: null
  closedReason: null
```

-----

## 11. Crontab

**Location:** `/etc/avix/crontab.yaml`

Defines scheduled agent invocations. Uses standard cron expressions (5-field, UTC unless
`timezone` is set). The kernel spawns a fresh agent instance per job run.

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

## 12. Snapshot

**Location:** `/var/avix/snapshots/<name>.yaml`

A point-in-time serialisation of a running agent’s full state. Created on `SIGSAVE`,
`autoSnapshotInterval`, or by a user command. Used for crash recovery and agent cloning.

```yaml
apiVersion: avix/v1
kind: Snapshot
metadata:
  name: researcher-20260315-0741
  agentName: researcher
  sourcePid: 57
  capturedAt: 2026-03-15T07:41:00-05:00
  capturedBy: kernel            # kernel | user:<uid> | agent:<pid>
  trigger: auto                 # auto | crash | manual | sigsave

spec:
  goal: "Research quantum computing breakthroughs 2025"
  contextSummary: |
    Agent has completed web search phase. Found 12 sources.
    Currently synthesising findings. 3 tool calls remaining in chain.
  contextTokenCount: 64000

  memory:
    episodicEvents: 14
    semanticKeys: 8

  pendingRequests:
    - requestId: req-abc124
      resource: tool
      name: web
      status: in-flight         # in-flight requests are re-issued on restore

  pipes:
    - pipeId: pipe-001
      state: open               # restored pipes are reconnected if target still running

  environment:
    temperature: 0.7
    capabilityToken: sha256:tokenSig789...

  checksum: sha256:snap001...   # integrity check verified by kernel on restore
```

-----

## 13. SessionManifest

**Location:** `/var/avix/sessions/<session-id>.yaml`

Tracks an interactive session — a user-facing context in which one or more agents may
run. Sessions correspond to shell logins, API connections, or UI conversations.

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
  workingDirectory: /home/alice

  agents:
    - pid: 57
      name: researcher
      role: primary
    - pid: 58
      name: writer
      role: subordinate

  quotaSnapshot:                # snapshot of quota state at session open
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

## Appendix A — Capability Reference

All capability names used in `AgentManifest.spec.capabilities`,
`Users.spec.users[].capabilities`, and `CapabilityToken.spec.capabilities`.

|Capability |Description                                            |Risk Level|
|-----------|-------------------------------------------------------|----------|
|`web`      |HTTP fetch / web search                                |Medium    |
|`read`     |Read files from MemFS paths the agent has ACL access to|Low       |
|`write`    |Write files to MemFS paths the agent has ACL access to |Medium    |
|`file_io`  |Full file read + write (broader than `read`/`write`)   |Medium    |
|`code_exec`|Execute sandboxed code (Python, shell)                 |High      |
|`email`    |Send email via configured mail transport               |High      |
|`db`       |Query/write configured database connections            |High      |
|`pipe`     |Create or receive pipes to/from other agents           |Medium    |
|`spawn`    |Spawn child agents                                     |High      |
|`snapshot` |Read/write snapshots in `/var/avix/snapshots/`         |Medium    |
|`all`      |Superuser; all capabilities (root only)                |Critical  |

-----

## Appendix B — Filesystem Path Reference

```
/bin/<agent>/           Agent executables and manifests
/installed/<agent>/     User-installed agents
/proc/<pid>/            Per-agent runtime state (status, pipes, hil-queue)
/etc/avix/              System configuration (kernel, users, crews, crontab)
/home/<user>/           User home directories
/srv/                   Service account working directories
/shared/                Shared memory accessible by crew (ACL-controlled)
/var/avix/snapshots/    Agent snapshots
/var/avix/sessions/     Session manifests
/var/log/avix/          Kernel and agent logs
/var/run/avix/          Runtime sockets and PID files
/tools/                 Registered tool definitions
/agents/                Symlinks to running agent procs (like /proc aliases)
/kernel/                Kernel internals (read-only for non-root)
```

-----

## Appendix C — Validation Rules (Summary)

The Avix CLI (`avix validate <file>`) enforces these rules:

1. Every file **must** have `apiVersion: avix/v1` and a valid `kind`.
1. `metadata.name` must match `^[a-z0-9][a-z0-9\-]{1,62}$` where required.
1. `spec` is required and must not be empty.
1. Unknown top-level fields cause a validation error.
1. All timestamps must be ISO 8601 with timezone offset.
1. `CapabilityToken.spec.signature` must be verified before any ResourceRequest is honoured.
1. `Snapshot.checksum` must be verified before restore.
1. `Crontab` schedule expressions are validated as standard 5-field cron (UTC).
1. `Users` UIDs below 1000 are reserved; validator warns on manual assignment.
1. `AgentManifest.spec.capabilities.required` must be a strict subset of the spawning
   user’s `capabilities` list, otherwise spawn is rejected.
