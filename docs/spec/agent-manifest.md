# AgentManifest

← [Back to Schema Index](./README.md)

**Kind:** `AgentManifest`  
**Location:** `/bin/<agent>/manifest.yaml` (system) or `/users/<username>/bin/<agent>/manifest.yaml` (user-installed)  
**Direction:** Config (static, immutable after install)

Defines an agent’s static identity, tool requirements, and default behaviour. This is the
application metadata created by the agent developer, packaged with prompt files, and
installed as an immutable bundle. Users spawn instances from this manifest; the manifest
itself never changes at runtime.

-----

## Philosophy

Agents in Avix follow an **app installation model**:

1. Creators design agents for specific tasks, tune parameters, write prompts, and package
   everything into a signed bundle
1. Users install agents once (`avix install researcher`) into `/bin/`
1. Users spawn multiple instances (`avix spawn researcher --goal "..."`) with different
   runtime goals
1. Each instance is a separate process (PID) referencing the same immutable manifest

The manifest is analogous to a mobile app’s metadata file or a Docker image definition —
it describes what the agent is, what it needs, and how it should run, but contains no
runtime state.

**Security model:** tools are the primary security boundary. Access control is managed
through:

- Manifest declares `required` and `optional` tools
- User ACLs (`/etc/avix/users.yaml`) define what tools a user can grant
- Crews (`/crews/<crew-name>/`) provide reusable tool bundles for teams
- Kernel enforces the intersection at spawn time

-----

## Schema

```yaml
apiVersion: avix/v1
kind: AgentManifest

metadata:
  name: researcher
  version: 1.3.0
  compatibilityVersion: 1          # increment on breaking changes
  description: General-purpose web & document researcher
  author: kernel-team
  createdAt: 2026-03-10T14:22:00Z
  license: MIT                     # optional — SPDX identifier
  signature: sha256:abc123def456...  # package integrity hash; verified at install and spawn

spec:
  entrypoint:
    type: llm-loop                 # only supported type currently

    modelRequirements:
      minContextWindow: 32000      # minimum context window in tokens
      requiredCapabilities:        # model features this agent depends on
        - tool_use
        - vision                   # optional: only for image analysis agents
      recommended: claude-sonnet-4 # model the creator tested and optimised with

    maxToolChain: 8                # max sequential tool calls per LLM turn
    maxTurnsPerGoal: 50            # max conversation turns before forced termination

  tools:
    required:                      # spawn fails if any of these are unavailable or denied
      - web_search
      - web_fetch
      - file_read
    optional:                      # nice-to-have; agent degrades gracefully if absent
      - code_interpreter
      - send_email
    # Built-in kernel tools are always available — no declaration needed:
    # cli.print, cli.readline, cli.args, gui.show, gui.ask,
    # signal.pause, signal.resume

  memory:
    workingContext: dynamic        # fixed | dynamic
    episodicPersistence: true      # write conversation turns to /memory/<pid>/episodes/
    semanticStoreAccess: read-only # none | read-only | read-write

  snapshot:
    mode: per-turn                 # per-turn | disabled
    restoreOnCrash: true           # kernel auto-respawns from last snapshot on crash
    compressionEnabled: true       # compress snapshot files to reduce storage

  defaults:
    systemPrompt: |                # embedded from prompts/system.md at build time
      You are a research assistant specializing in gathering,
      analyzing, and synthesizing information from multiple sources.

    goalTemplate: |                # embedded from prompts/goal-template.md at build time
      Research and summarize: {{topic}}.
      Include sources and a confidence score.
      Format output as markdown with sections.

    environment:
      temperature: 0.7
      topP: 0.9
      timeoutSec: 300
```

-----

## Package Structure

Agents are distributed as signed tarballs:

```
researcher-1.3.0.tar.gz
├── manifest.yaml              # this file — all content baked in
├── prompts/
│   ├── system.md              # content embedded into defaults.systemPrompt
│   └── goal-template.md       # content embedded into defaults.goalTemplate
├── examples/                  # optional: few-shot examples embedded in system prompt
│   ├── research-paper.md
│   └── market-analysis.md
└── README.md                  # optional: documentation for users
```

**At package build time:**

- All `prompts/*.md` content is read and embedded directly into `manifest.yaml`
- File references like `{{file:prompts/system.md}}` are resolved and inlined
- Resulting manifest is hashed and signed
- Tarball contains the final manifest with all content baked in

**At install time:**

- Kernel verifies signature against manifest hash
- Extracts to `/bin/researcher@1.3.0/`
- Creates symlink `/bin/researcher → researcher@1.3.0`
- Manifest becomes immutable

Users cannot edit installed agents. To modify, they must get the source, edit prompts,
rebuild the package, and reinstall with a new version number.

-----

## Field Reference

### `metadata`

|Field                 |Type    |Required|Default|Description                                     |
|----------------------|--------|--------|-------|------------------------------------------------|
|`name`                |string  |yes     |—      |Agent identifier, e.g. `researcher`, `coder`    |
|`version`             |semver  |yes     |—      |Semantic version, e.g. `1.3.0`                  |
|`compatibilityVersion`|int     |no      |`1`    |Incremented on breaking changes                 |
|`description`         |string  |yes     |—      |One-line summary of agent purpose               |
|`author`              |string  |yes     |—      |Creator name or organisation                    |
|`createdAt`           |ISO 8601|yes     |—      |Package build timestamp                         |
|`license`             |string  |no      |—      |SPDX identifier, e.g. `MIT`, `Apache-2.0`       |
|`signature`           |string  |yes     |—      |SHA-256 hash of manifest + prompts for integrity|

### `spec.entrypoint`

|Field                                   |Type  |Default          |Description                                        |
|----------------------------------------|------|-----------------|---------------------------------------------------|
|`type`                                  |enum  |`llm-loop`       |Execution mode; only `llm-loop` supported currently|
|`modelRequirements.minContextWindow`    |int   |`8000`           |Minimum context window in tokens                   |
|`modelRequirements.requiredCapabilities`|list  |`[tool_use]`     |Model features required                            |
|`modelRequirements.recommended`         |string|`claude-sonnet-4`|Model creator tested with                          |
|`maxToolChain`                          |int   |`5`              |Max sequential tool calls per LLM turn             |
|`maxTurnsPerGoal`                       |int   |`20`             |Max turns before forced termination                |

**Model resolution at spawn:**

- User passes `--model` → kernel validates against `modelRequirements`
- User omits `--model` → kernel uses `KernelConfig.models.default`
- Selected model fails requirements → spawn rejected with helpful error

### `spec.tools`

Tools are the primary security boundary. They define what actions an agent can perform.

|Field     |Type|Default|Description                                                      |
|----------|----|-------|-----------------------------------------------------------------|
|`required`|list|`[]`   |Spawn fails if any are unavailable or denied by user ACL         |
|`optional`|list|`[]`   |Silently omitted if unavailable; agent handles absence gracefully|

**Built-in kernel tools** — always granted, no declaration needed:

|Tool           |Description                          |
|---------------|-------------------------------------|
|`cli.print`    |Write to stdout                      |
|`cli.readline` |Read from stdin                      |
|`cli.args`     |Access spawn arguments               |
|`gui.show`     |Display UI element (if GUI supported)|
|`gui.ask`      |Prompt user for input                |
|`signal.pause` |Request human approval (`SIGPAUSE`)  |
|`signal.resume`|Continue after pause                 |

**Common external tools:**

|Tool              |Description                 |Typical use            |
|------------------|----------------------------|-----------------------|
|`web_search`      |Search the web              |Research, fact-checking|
|`web_fetch`       |Fetch web page content      |Content analysis       |
|`http_request`    |Make arbitrary HTTP requests|API integration        |
|`file_read`       |Read files from workspace   |Document processing    |
|`file_write`      |Write files to workspace    |Report generation      |
|`bash`            |Execute bash commands       |System automation      |
|`python`          |Run Python code             |Data analysis          |
|`code_interpreter`|Interactive code execution  |Complex computation    |
|`send_email`      |Send email messages         |Notifications          |
|`read_inbox`      |Read email inbox            |Email processing       |
|`calendar_read`   |Read calendar events        |Scheduling             |
|`calendar_write`  |Create calendar events      |Meeting setup          |
|`git`             |Version control operations  |Code workflows         |

**Tool namespacing** — tools can be grouped by namespace:

- `fs.*` — filesystem operations (`fs.read`, `fs.write`, `fs.list`)
- `net.*` — network operations (`net.http`, `net.fetch`, `net.websocket`)
- `llm.*` — LLM operations (`llm.chat`, `llm.embed`, `llm.complete`)

Manifests may request a namespace wildcard:

```yaml
tools:
  required:
    - fs.read
    - net.*      # grants all net.* tools the user is permitted
```

**Tool resolution at spawn:**

```
1. Agent declares required and optional tools in manifest
2. Kernel checks user ACL in /etc/avix/users.yaml or assigned crew
3. Grants intersection of manifest request and user's permitted tools
4. Any required tool denied or missing → spawn fails with clear error
5. Any optional tool denied or missing → silently omitted
6. Built-in kernel tools always granted automatically
```

**Example resolution:**

```yaml
# Agent manifest
tools:
  required: [web_search, file_read]
  optional: [bash, send_email]

# Crew definition (/crews/research-crew/limits.yaml)
allowedTools:
  - web_search
  - web_fetch
  - file_read
  - file_write

# Result at spawn
# ✓ Granted: web_search, file_read
# ✗ Denied:  bash (not in crew), send_email (not in crew)
# → Spawn succeeds (all required tools granted)
# → Agent runs with: [web_search, file_read] + built-ins
```

### `spec.memory`

|Field                |Type|Default  |Description                                              |
|---------------------|----|---------|---------------------------------------------------------|
|`workingContext`     |enum|`dynamic`|`fixed` — set at spawn; `dynamic` — grows up to model max|
|`episodicPersistence`|bool|`false`  |Write each LLM turn to `/memory/<pid>/episodes/`         |
|`semanticStoreAccess`|enum|`none`   |Vector DB access: `none`, `read-only`, `read-write`      |

- **Episodic persistence** enables post-hoc analysis, debugging, audit trails, and replay for fine-tuning.
- **Semantic store `read-write`** allows agents to add embeddings to the long-term knowledge store — use cautiously as this persists across sessions.

### `spec.snapshot`

|Field               |Type|Default   |Description                                                      |
|--------------------|----|----------|-----------------------------------------------------------------|
|`mode`              |enum|`disabled`|`per-turn` — save after each LLM turn; `disabled` — no snapshots |
|`restoreOnCrash`    |bool|`false`   |Kernel auto-respawns from last snapshot on unexpected termination|
|`compressionEnabled`|bool|`true`    |Compress snapshot files; recommended for `per-turn` mode         |

- Snapshots are stored at `/users/<username>/snapshots/<pid>/turn-<n>.snap`
- Manual restore: `avix restore <pid> --from-snapshot turn-42`
- Auto restore: if `restoreOnCrash: true`, kernel respawns from last good snapshot on `SIGKILL`

### `spec.defaults`

|Field                    |Type  |Default|Description                                                                 |
|-------------------------|------|-------|----------------------------------------------------------------------------|
|`systemPrompt`           |string|—      |Base agent instructions; embedded from `prompts/system.md` at build time    |
|`goalTemplate`           |string|—      |Goal template with `{{variables}}`; embedded from `prompts/goal-template.md`|
|`environment.temperature`|float |`0.7`  |Sampling temperature (0.0–1.0)                                              |
|`environment.topP`       |float |`0.9`  |Nucleus sampling parameter (0.0–1.0)                                        |
|`environment.timeoutSec` |int   |`300`  |Max seconds per LLM turn                                                    |

**Template variables** — substituted at spawn time from CLI args or pipe input:

- `{{topic}}`, `{{query}}`, `{{goal}}`, `{{task}}` — common goal variables
- Any key passed via `avix spawn --var key=value` is available as `{{key}}`

-----

## Versioning and Installation

### Version Format

Agents follow semantic versioning (`MAJOR.MINOR.PATCH`):

- **MAJOR** — breaking changes; incompatible with previous versions
- **MINOR** — new features; backward-compatible
- **PATCH** — bug fixes; backward-compatible

`metadata.compatibilityVersion` tracks breaking changes independently of semver,
allowing `researcher@1.x.x` and `researcher@2.x.x` to coexist as separate agents.

### Installation Paths

```
/bin/
  researcher/                    # symlink → researcher@1.3.0/
  researcher@1.3.0/              # installed stable version
    manifest.yaml
  researcher@1.4.0-beta/         # explicitly installed beta
    manifest.yaml
  researcher-v2/                 # major rewrite — installed as separate agent
    manifest.yaml
```

### Installation Commands

```sh
# Install latest stable
avix install researcher
# → /bin/researcher@1.3.0/ + symlink /bin/researcher → researcher@1.3.0

# Install specific version
avix install researcher@1.4.0-beta
# → /bin/researcher@1.4.0-beta/ — does not move symlink

# Upgrade default
avix install researcher --upgrade
# → installs latest, repoints symlink
# → running instances on old version continue unaffected

# Spawn specific version
avix spawn researcher@1.4.0-beta --goal "test new features"

# Spawn with model and variable overrides
avix spawn researcher --model claude-opus-4 --var topic="quantum computing"
```

-----

## Spawn-Time Resolution

When a user runs `avix spawn researcher --goal "..."`:

```
1. Manifest loading
   → Read /bin/researcher/manifest.yaml
   → Verify signature against stored hash — fail if tampered

2. Model selection
   → User passed --model?  validate against modelRequirements
   → No --model?           use KernelConfig.models.default
   → Model meets requirements? proceed — else reject with helpful message

3. Tool grant
   → Load user ACL from /etc/avix/users.yaml and crew limits
   → Grant intersection of manifest tools and user's permitted tools
   → Any required tool denied? reject spawn with clear error
   → Any optional tool denied? silently omit from agent's tool list
   → Always grant built-in kernel tools

4. Instance creation
   → Allocate PID
   → Create /proc/<pid>/ directory structure
   → Write AgentStatus with granted tools and state: pending
   → Apply resolved config (defaults + limits merge)
   → Inject systemPrompt and rendered goalTemplate into LLM context
   → Send SIGSTART → state: running
```

-----

## Access Control via Crews

Users can be assigned to crews that define reusable tool bundles:

```yaml
# /crews/research-crew/limits.yaml
apiVersion: avix/v1
kind: Limits
metadata:
  target: tools
  layer: crew
  owner: research-crew

limits:
  allowedTools:
    - web_search
    - web_fetch
    - file_read
    - file_write
  deniedTools:
    - bash
    - send_email
```

```yaml
# /etc/avix/users.yaml (excerpt)
- username: alice
  crews: [research-crew]
  additionalTools:      # user-specific additions on top of crew
    - python
  deniedTools:          # user-specific restrictions on top of crew
    - file_write
```

**Resolution priority:**

1. Crew provides base allowed tool set
1. User’s `additionalTools` are added
1. User’s `deniedTools` are removed
1. Result intersected with agent manifest’s declared tools

-----

## Example Manifests

### Minimal Agent

```yaml
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent for testing
  author: avix-core
  createdAt: 2026-03-15T10:00:00Z
  signature: sha256:minimal...

spec:
  entrypoint:
    type: llm-loop

  defaults:
    systemPrompt: "You are a helpful assistant that echoes user input."
```

Uses all kernel defaults. Requires no tools beyond built-ins.

### Research Agent

```yaml
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: researcher
  version: 1.3.0
  compatibilityVersion: 1
  description: General-purpose web & document researcher
  author: kernel-team
  createdAt: 2026-03-10T14:22:00Z
  license: MIT
  signature: sha256:abc123def456...

spec:
  entrypoint:
    type: llm-loop
    modelRequirements:
      minContextWindow: 32000
      requiredCapabilities: [tool_use]
      recommended: claude-sonnet-4
    maxToolChain: 8
    maxTurnsPerGoal: 50

  tools:
    required:
      - web_search
      - web_fetch
      - file_read
    optional:
      - code_interpreter
      - python

  memory:
    workingContext: dynamic
    episodicPersistence: true
    semanticStoreAccess: read-only

  snapshot:
    mode: per-turn
    restoreOnCrash: true
    compressionEnabled: true

  defaults:
    systemPrompt: |
      You are a research assistant specializing in gathering,
      analyzing, and synthesizing information from multiple sources.
      Your outputs should be comprehensive, well-sourced, and
      include confidence scores for key claims.

    goalTemplate: |
      Research and summarize: {{topic}}.

      Requirements:
      - Cite all sources with URLs
      - Provide confidence score (0-100) for main claims
      - Format as markdown with sections:
        ## Summary
        ## Key Findings
        ## Sources
        ## Confidence Assessment

    environment:
      temperature: 0.7
      topP: 0.9
      timeoutSec: 300
```

### Code Assistant

```yaml
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: coder
  version: 2.1.0
  compatibilityVersion: 2
  description: Code generation and debugging assistant
  author: dev-tools-team
  createdAt: 2026-03-12T08:00:00Z
  license: Apache-2.0
  signature: sha256:coder789...

spec:
  entrypoint:
    type: llm-loop
    modelRequirements:
      minContextWindow: 64000      # large context for codebases
      requiredCapabilities: [tool_use]
      recommended: claude-opus-4
    maxToolChain: 15               # complex debugging workflows
    maxTurnsPerGoal: 100

  tools:
    required:
      - file_read
      - file_write
      - bash
      - python
    optional:
      - web_search                 # looking up docs
      - git                        # version control operations

  memory:
    workingContext: dynamic
    episodicPersistence: true
    semanticStoreAccess: read-write  # learn from codebase

  snapshot:
    mode: per-turn
    restoreOnCrash: true
    compressionEnabled: true

  defaults:
    systemPrompt: |
      You are an expert software engineer assistant.
      You write clean, well-tested, production-quality code.
      You explain your reasoning and consider edge cases.

    goalTemplate: |
      {{task}}

      Code style preferences:
      - Follow project conventions
      - Include type hints (Python) or types (TypeScript)
      - Write tests for new functionality
      - Add docstrings/comments for complex logic

    environment:
      temperature: 0.3             # lower = more deterministic code output
      topP: 0.95
      timeoutSec: 600              # longer timeout for complex tasks
```

-----

## Field Defaults

System defaults live at `/kernel/defaults/agent-manifest.yaml`.

|Field                                              |Default          |Notes                        |
|---------------------------------------------------|-----------------|-----------------------------|
|`entrypoint.type`                                  |`llm-loop`       |Only supported type currently|
|`entrypoint.modelRequirements.minContextWindow`    |`8000`           |Baseline for simple agents   |
|`entrypoint.modelRequirements.requiredCapabilities`|`[tool_use]`     |LLM must support tool calling|
|`entrypoint.modelRequirements.recommended`         |`claude-sonnet-4`|System default model         |
|`entrypoint.maxToolChain`                          |`5`              |Conservative default         |
|`entrypoint.maxTurnsPerGoal`                       |`20`             |Prevents runaway loops       |
|`tools.required`                                   |`[]`             |No tools required by default |
|`tools.optional`                                   |`[]`             |No optional tools            |
|`memory.workingContext`                            |`dynamic`        |Context grows as needed      |
|`memory.episodicPersistence`                       |`false`          |Don’t persist by default     |
|`memory.semanticStoreAccess`                       |`none`           |No vector DB access          |
|`snapshot.mode`                                    |`disabled`       |No snapshots by default      |
|`snapshot.restoreOnCrash`                          |`false`          |Don’t auto-restore           |
|`snapshot.compressionEnabled`                      |`true`           |Compress if snapshots enabled|
|`defaults.environment.temperature`                 |`0.7`            |Balanced creativity          |
|`defaults.environment.topP`                        |`0.9`            |Standard nucleus sampling    |
|`defaults.environment.timeoutSec`                  |`300`            |5 minutes per turn           |

See [Resolved](./resolved.md) for how these merge with user and crew defaults.

-----

## Security Considerations

**Package integrity** — manifests are SHA-256 signed. The kernel verifies the signature
at both install time and spawn time. Tampering causes spawn rejection.

**Tool-based access control** — tools are the sole security boundary between an agent
and the system. Agents cannot escalate their own tool access; they can only use what was
granted at spawn. The principle of least privilege applies: `required` should be the
minimum needed for the agent to function.

**Tool sandboxing** — external tools (`bash`, `python`) run in isolated environments.
Filesystem access is limited to the agent’s workspace and permitted shared paths.
Network tools can be rate-limited or filtered by kernel policy.

**Audit trail** — episodic memory and snapshot history provide a forensic record of
agent behaviour. Tool usage is tracked per-PID.

**Crew isolation** — users in different crews cannot grant each other’s tools. Crew
`deniedTools` cannot be overridden by individual users within that crew.

-----

## Related

- [AgentStatus](./agent-status.md) — runtime state for a spawned instance
- [CapabilityToken](./capability-token.md) — token issued at spawn encoding granted tools
- [Snapshot](./snapshot.md) — controlled by `spec.snapshot`
- [Crews](./crews.md) — reusable tool bundles; referenced in access control
- [KernelConfig](./kernel-config.md) — system-wide model defaults and limits
- [Defaults](./defaults.md) — layered fallback values for unset fields
- [Limits](./limits.md) — bounds that constrain what values this manifest may set
- [Resolved](./resolved.md) — the final merged config this agent actually runs with
