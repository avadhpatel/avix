# Avix Memory Architecture Spec

← Back to Schema Index

**Kind:** `MemoryConfig` (kernel config section) + `MemoryRecord` (VFS document)
**Service:** `memory.svc`
**Config location:** `/etc/avix/kernel.yaml` (`memory:` stanza)
**Runtime state:** `/proc/services/memory/`
**Persistent store:** `/users/<username>/memory/<agent-name>/`

-----

## Overview

Memory in Avix is the mechanism by which agents accumulate knowledge across turns and
sessions, improve over time for a specific user, and — with explicit human approval —
share that knowledge with other agents or crews.

The memory subsystem is built on four principles:

1. **Text is the source of truth.** Every stored memory record is a human-readable
   summary produced by the agent’s own LLM from real session content. You can open any
   `.yaml` in the memory tree and understand it without tooling. Vectors and indexes are
   derived from this text — they can always be rebuilt. The text cannot be lost.
1. **The agent’s model is the intelligence layer on both sides.** The agent LLM decides
   what matters and writes a meaningful summary (write path). `memory.svc` uses that same
   model to semantically match a natural language query against stored summaries (read
   path). `memory.svc` is not an independent reasoning system — it is a managed store
   that borrows the agent’s model for retrieval quality.
1. **Memory is a service, not a filesystem.** Agents do not read the memory tree
   directly. All operations go through `memory.svc` tools, which enforce ACL, invoke the
   model for retrieval, and maintain indexes. The VFS tree is the backing store, not the
   interface.
1. **Sharing requires HIL.** Memory grant requests follow the same `ApprovalToken` +
   `SIGPAUSE` / `SIGRESUME` pattern as capability upgrades. No agent can read another
   agent’s memories — or a crew’s shared memory — without a human explicitly approving
   the grant.

-----

## Memory Taxonomy

Three memory types are in scope for v1. Each has a different persistence scope, write
semantics, and retrieval pattern.

### 1. Episodic Memory

A time-ordered log of significant events from an agent’s execution history. Analogous
to a diary: what the agent did, what it found, what decisions it made, what outcomes
resulted, and how the user responded.

- **Written by:** the agent LLM, which decides what is significant and produces the
  summary; stored via `memory/log-event`
- **Read by:** the owning agent via `memory/retrieve`; `memory.svc` at spawn for
  context injection
- **Persisted:** yes — survives agent death and system restart
- **Retention:** `kernel.yaml: memory.episodic.maxRetentionDays` (default 30);
  compacted by `memory-gc-daily`

**Primary UX value:** gives agents continuity across sessions. “Last time you asked me
to analyse the Q3 report, I flagged three OPEX anomalies and you approved the finding.”

### 2. Semantic Memory

Distilled, structured facts the agent has chosen to retain long-term. Not a log of
events — a key-addressable knowledge store of things the agent has learned and wants
to keep.

- **Written by:** the agent LLM via `memory/store-fact`; the agent decides the key,
  value, and confidence based on session context
- **Read by:** owning agent via `memory/retrieve` or exact-key `memory/get-fact`
- **Persisted:** yes — no automatic expiry; pruned or updated by the agent
- **Retention:** no TTL by default; agent manages its own semantic store

**Primary UX value:** persistent knowledge that transcends any single session.
“I know alice’s project-alpha deadline is April 30.” “I know this team’s preferred
report template is at `/users/alice/workspace/templates/report.md`.”

### 3. User Preference Memory

A specialised sub-type of semantic memory scoped to an agent’s learned model of its
user — communication style, recurring preferences, corrections, domain context. Lives
in a dedicated namespace for easy auditing and is always injected at spawn.

- **Written by:** the agent LLM via `memory/update-preference`; populated when the
  agent observes a preference signal (user corrects output, states a preference, or
  the agent infers a pattern)
- **Read by:** owning agent; automatically injected into the system prompt at spawn
  by `memory.svc` before the LLM sees the first user message
- **Persisted:** yes — no automatic expiry

**Primary UX value:** the primary driver of “gets smarter about you over time.” An
agent that has learned you prefer markdown tables, work in America/New_York, and always
want sources cited will produce materially better output from the first message of
every session — without you repeating yourself.

-----

## VFS Layout

```
/users/<username>/memory/
└── <agent-name>/
    ├── episodic/
    │   ├── 2026-03-22T14:30:00Z-abc123.yaml    # MemoryRecord (episodic)
    │   ├── 2026-03-22T15:10:00Z-def456.yaml
    │   └── index/
    │       ├── fulltext.idx                     # BM25 — maintained by memory.svc
    │       └── vectors.idx                      # derived from text — maintained by memory.svc
    ├── semantic/
    │   ├── <fact-key>.yaml                      # MemoryRecord (semantic)
    │   └── index/
    │       ├── fulltext.idx
    │       └── vectors.idx
    └── preferences/
        └── user-model.yaml                      # UserPreferenceModel

/crews/<crew-name>/memory/                       # crew shared memory (same structure)
    └── shared/
        ├── episodic/
        ├── semantic/
        └── index/
```

**Key rules:**

- Paths are owned by `<username>` UID. No other user’s agents or services can read
  another user’s memory tree.
- Within a user tree, each `<agent-name>` directory is further scoped: `memory.svc`
  enforces that only the agent matching that name can write to it.
- Crew memory at `/crews/<crew-name>/memory/` is readable by agents that are members
  of that crew, subject to the same `memory:read` capability check. Writes to crew
  memory require `memory:write` and crew membership.
- Index files under `index/` are written exclusively by `memory.svc`. Agents may not
  call `fs/write` on any path under `memory/`. All writes go through `memory.svc` tools.
- For services, `/services/<svcname>/memory/` mirrors the user layout exactly.

-----

## MemoryRecord Schema

Every record stored in the memory tree is a `MemoryRecord`. The `spec.content` field
is always human-readable text — a summary produced by the agent’s LLM. The `spec.index`
block is metadata maintained by `memory.svc` to support retrieval; agents never write
it directly.

```yaml
apiVersion: avix/v1
kind: MemoryRecord

metadata:
  id: mem-abc123
  type: episodic                # episodic | semantic
  agentName: researcher
  agentPid: 57                  # informational — pid of the writing agent
  owner: alice
  createdAt: 2026-03-22T14:30:00Z
  updatedAt: 2026-03-22T14:30:00Z
  sessionId: sess-xyz789
  tags: [research, web, quantum]
  pinned: false                 # pinned: true records are always injected at spawn

spec:
  # ── Episodic record ─────────────────────────────────────────────────────
  content: >
    Completed web research phase on quantum computing breakthroughs 2025.
    Found 12 sources across arxiv, MIT News, and Google AI Blog. High-confidence
    finding: three independent papers on topological qubits, all published Q1 2026,
    suggest error correction thresholds are near practical viability. User reviewed
    the final report and approved without edits. Suggested following up on the
    Google paper in the next session.
  outcome: success              # success | partial | failure — episodic only
  relatedGoal: "Research quantum computing breakthroughs 2025"
  toolsUsed: [web_search, web_fetch]

  # ── Semantic record (alternative shape) ─────────────────────────────────
  # content: "Project Alpha deadline is April 30, 2026. Confirmed directly by
  #   alice on 2026-03-22 during project kickoff session."
  # key: project-alpha-deadline           # unique within agent's semantic store
  # confidence: high                      # high | medium | low
  # ttlDays: null                         # null = no expiry

  # ── Index metadata — written by memory.svc, never by agents ─────────────
  index:
    vectorModel: text-embedding-3-small   # which model produced the vector
    vectorUpdatedAt: 2026-03-22T14:30:05Z
    fulltextUpdatedAt: 2026-03-22T14:30:05Z
```

### Why the index block lives on the record

Embedding model version is stored per-record, not globally. When the default embedding
model changes, `memory.svc` identifies exactly which records need re-vectorisation by
comparing `index.vectorModel` against the current configured model. Records with a
matching model are skipped. The re-index job only processes the delta.

Since the canonical content is always the human-readable `spec.content` text,
re-vectorisation is always lossless: re-embed the text, write the new vector, update
`index.vectorModel`. No data loss is possible — the text is the truth.

-----

## UserPreferenceModel Schema

```yaml
apiVersion: avix/v1
kind: UserPreferenceModel

metadata:
  agentName: researcher
  owner: alice
  updatedAt: 2026-03-22T15:00:00Z

spec:
  # Free-text summary — produced by the agent LLM, updated over time.
  # This is what gets injected verbatim into the system prompt at spawn.
  summary: >
    Alice prefers concise responses in markdown format. Always cite sources.
    Use tables for comparisons — she has corrected prose-formatted comparisons
    twice. She works in America/New_York and usually engages between 09:00-18:00.
    Domain background: distributed systems, Rust, LLM architecture. She is
    comfortable with technical depth and dislikes over-explanation.

  # Structured fields — used for programmatic preference checks and
  # deterministic spawn injection alongside the prose summary.
  structured:
    outputFormat: markdown
    preferredLength: concise
    citeSources: always
    tonePreference: professional
    timezone: America/New_York
    primaryLanguage: en
    expertiseAreas: [distributed-systems, rust, llm-architecture]
    proactiveUpdates: true

  corrections:
    # Recorded instances where the user corrected agent output.
    # Injected as few-shot examples at spawn.
    - at: 2026-03-20T10:00:00Z
      context: "Agent formatted a feature comparison as prose paragraphs"
      correction: "Please use a markdown table for comparisons"
    - at: 2026-03-18T14:00:00Z
      context: "Agent gave a 5-paragraph explanation of a basic concept"
      correction: "Assume I know this — skip the background"
```

The `summary` field is the primary injection artifact — written into the system prompt
verbatim at spawn. The `structured` fields provide machine-readable access to the same
information for agents that want to programmatically check a preference mid-session.

-----

## memory.svc

`memory.svc` is a built-in core service. It is the exclusive writer to all memory
trees in the VFS. It holds no LLM capability of its own — all inference calls are
made on behalf of the calling agent, using that agent’s model and charged against
that agent’s quota via the `_caller` token pass-through.

### Responsibilities

- Store and index `MemoryRecord` and `UserPreferenceModel` documents
- Serve natural language retrieval queries using the calling agent’s assigned model
- Maintain BM25 full-text indexes and vector indexes as retrieval optimizations
- Inject preference memory and recent episodic context into agent system prompts at spawn
- Enforce per-agent namespace isolation — ACL check on every read/write
- Mediate all memory share and crew memory access requests
- Expose runtime state at `/proc/services/memory/`

### Retrieval Model

When an agent calls `memory/retrieve` with a natural language query, `memory.svc`
uses the agent’s assigned model as the ranking intelligence:

```
1. Candidate fetch (parallel)
   ├── BM25 full-text search over spec.content
   │     → top-K candidates by keyword relevance
   └── Vector cosine search over vectors.idx
         → top-K candidates by semantic similarity

2. Candidate merge
   └── RRF (Reciprocal Rank Fusion, k=60) over both lists
         → unified ranked candidate list (deduped)

3. Model re-rank  ← the intelligence step
   └── memory.svc calls llm/complete with the calling agent's assigned model:
         System: "You are a memory retrieval assistant. Rank these memory records
                  by relevance to the query. Return only a JSON array of IDs in
                  ranked order. Exclude records with no meaningful relevance."
         User:   "Query: <agent's natural language query>
                  Candidates:
                    [mem-abc123] Completed web research on quantum computing...
                    [mem-def456] Analysed Q3 financials, found 3 OPEX anomalies...
                    ..."
         → model returns ordered IDs with optional per-record relevance note

4. Return
   └── Ranked MemoryRecord stubs returned to agent
       (id, type, scope, summary snippet, relevance note, tags, createdAt)
```

**Model and quota pass-through:** `memory.svc` is `caller_scoped: true`. When it
calls `llm/complete` for the re-rank step, it forwards the calling agent’s
`CapabilityToken` as the `_caller` context. `llm.svc` resolves the agent’s
`modelPreference` from `/proc/<pid>/resolved.yaml`, uses that model, and charges
the tokens against the agent’s quota — not `memory.svc`‘s. `memory.svc` has no
LLM quota of its own. The retrieval prompt is short and deterministic — no tool
use, no multi-turn — so the token cost is small relative to the agent’s primary
task work, and fully visible in the agent’s own usage accounting.

**Why the model re-ranks:** BM25 and vector similarity catch candidates well but rank
by surface form and embedding proximity, not semantic intent. The model re-rank step
lets the agent’s own reasoning capability answer “is this record actually relevant to
my query?” — retrieval quality tracks the agent’s intelligence level, not a fixed
algorithm’s.

**Graceful degradation — stale vector index:** if the vector index is stale (embedding
model mismatch), `memory.svc` skips the vector step and passes BM25-only candidates
into the model re-rank. Retrieval degrades in recall, not correctness. The weekly
reindex job restores full quality.

**Graceful degradation — agent lacks `llm:inference`:** if the calling agent’s
`CapabilityToken` does not include `llm:inference`, `memory.svc` skips the model
re-rank entirely and returns the RRF-merged BM25 + vector candidates directly, ordered
by RRF score. The `relevance` field is omitted from results. The tool call succeeds —
retrieval is keyword/vector quality rather than model quality.

### Write Model

`memory.svc` is a dumb librarian on the write path. It stores exactly what the agent
sends, indexes it, and returns a confirmation. It does not summarise, rewrite, or
validate content quality — that is entirely the agent LLM’s responsibility.

This means:

- Agents call memory write tools with already-composed, human-readable summaries —
  not raw tool outputs or transcript excerpts.
- RuntimeExecutor auto-triggers `memory/log-event` at session end when
  `autoLogOnSessionEnd: true`. At that point, the agent LLM first composes a session
  summary, then the RuntimeExecutor calls `memory/log-event` with that summary.
- Mid-session writes (e.g., storing a discovered fact) are agent-initiated:
  the agent decides what matters and composes the content before calling
  `memory/store-fact`.

-----

## Tool Surface — /tools/memory/

All tools require `memory:read` or `memory:write` capability. These are granted at
spawn from the `AgentManifest` memory block, intersected with user/crew ACL — exactly
like all other capabilities.

-----

### `memory/retrieve`

Query across the agent’s episodic memory, semantic memory, crew shared memory, and
any active `MemoryGrant` records — in a single call. Results are re-ranked by the
agent’s model. This is the primary tool an agent uses to ask: “what do I know or
remember that’s relevant to what I’m about to do?”

**Capability required:** `memory:read`

**Input:**

```json
{
  "query":  "alice's preferences for report formatting",
  "scopes": ["own", "crew", "grants"],   // default: all three the agent has access to
  "types":  ["episodic", "semantic"],    // default: both
  "limit":  5,                           // default: 5, max: 20
  "since":  "2026-01-01T00:00:00Z"       // optional — episodic date filter
}
```

**Output:**

```json
{
  "records": [
    {
      "id":        "mem-abc123",
      "type":      "episodic",
      "scope":     "own",
      "summary":   "Completed web research on quantum computing. Found 12 sources...",
      "relevance": "High — agent had previous formatting corrections in this session",
      "tags":      ["research", "quantum"],
      "createdAt": "2026-03-22T14:30:00Z",
      "pinned":    false
    },
    {
      "id":        "mem-crew-001",
      "type":      "semantic",
      "scope":     "crew:researchers",
      "summary":   "Team standard report template is at /users/shared/templates/report.md",
      "relevance": "Medium — related to report structure and formatting",
      "tags":      ["template", "reporting"],
      "createdAt": "2026-03-10T09:00:00Z",
      "pinned":    true
    }
  ],
  "totalCandidates": 34,
  "returned": 2
}
```

The `relevance` field is the model’s explanation of why the record was returned. This
gives the calling agent — and any human reading a trace — transparent reasoning for
each result.

-----

### `memory/log-event`

Store an episodic memory record. The agent LLM composes the summary before calling
this tool. `memory.svc` stores it verbatim and indexes it.

**Capability required:** `memory:write`

**Input:**

```json
{
  "summary":     "Completed analysis of Q3 financials. Identified 3 anomalies in OPEX lines 7, 12, and 19. Alice reviewed and confirmed all three. Report delivered to /users/alice/workspace/q3-report.md. Alice requested a follow-up on line 19 next session.",
  "outcome":     "success",
  "relatedGoal": "Analyse Q3 financial report",
  "tags":        ["finance", "q3", "anomaly"],
  "pinned":      false,
  "scope":       "own"           // own | crew:<crew-name>
}
```

**Output:**

```json
{
  "id":      "mem-def456",
  "stored":  true,
  "indexed": true
}
```

**`scope: crew:<crew-name>`** stores the record in the crew’s shared memory tree rather
than the agent’s private tree. Requires `memory:write` + crew membership. No HIL is
required for writing to a crew the agent already belongs to.

-----

### `memory/store-fact`

Write or update a semantic memory record with a named key.

**Capability required:** `memory:write`

**Input:**

```json
{
  "key":        "project-alpha-deadline",
  "summary":    "Project Alpha deadline is April 30, 2026. Confirmed directly by alice during the kickoff session on 2026-03-22. Hard deadline — no extension expected. Key milestones: design review April 15, staging deploy April 25.",
  "confidence": "high",
  "tags":       ["project-alpha", "deadline"],
  "pinned":     true,
  "ttlDays":    null,
  "scope":      "own"
}
```

**Output:**

```json
{
  "id":       "mem-ghi789",
  "key":      "project-alpha-deadline",
  "stored":   true,
  "replaced": false
}
```

-----

### `memory/get-fact`

Retrieve a semantic record by exact key. Bypasses model retrieval — deterministic
lookup used when the agent already knows the key it wants.

**Capability required:** `memory:read`

**Input:**

```json
{
  "key":   "project-alpha-deadline",
  "scope": "own"
}
```

**Output:**

```json
{
  "found":  true,
  "record": {
    "id":         "mem-ghi789",
    "key":        "project-alpha-deadline",
    "summary":    "Project Alpha deadline is April 30, 2026...",
    "confidence": "high",
    "updatedAt":  "2026-03-22T14:00:00Z",
    "pinned":     true
  }
}
```

-----

### `memory/update-preference`

Update the `UserPreferenceModel`. Fields are merged — unspecified fields retain their
current value. The agent LLM produces both the prose `summary` and any `structured`
updates before calling this tool.

**Capability required:** `memory:write`

**Input:**

```json
{
  "summary": "Alice prefers concise responses in markdown. Always cite sources. Use tables for comparisons — corrected twice. Works America/New_York 09:00-18:00. Strong background in distributed systems and Rust. Assumes technical depth; dislikes over-explanation.",
  "structured": {
    "outputFormat": "markdown",
    "citeSources":  "always",
    "timezone":     "America/New_York"
  },
  "corrections": [
    {
      "context":    "Agent formatted a feature comparison as prose paragraphs",
      "correction": "Please use a markdown table for comparisons"
    }
  ]
}
```

**Output:**

```json
{
  "updated": true
}
```

-----

### `memory/get-preferences`

Read the current `UserPreferenceModel`. Agents call this mid-session to check a
specific preference programmatically.

**Capability required:** `memory:read`

**Output:**

```json
{
  "found": true,
  "model": { ... }
}
```

-----

### `memory/forget`

Delete one or more memory records by ID. Episodic and semantic only — preferences
are updated via `memory/update-preference`.

**Capability required:** `memory:write`

**Input:**

```json
{
  "ids":    ["mem-abc123", "mem-def456"],
  "reason": "User requested removal of session data from March 20"
}
```

**Output:**

```json
{
  "deleted":  ["mem-abc123", "mem-def456"],
  "notFound": []
}
```

-----

### `memory/share-request`

Request that one or more memory records be shared with a specific agent. Always
triggers HIL — the human approves before any grant is created.

**Capability required:** `memory:share` (privilege-level; not granted by default)

See **Memory Sharing — HIL Flow** below.

-----

## Auto-Injection at Spawn

When the kernel spawns an agent, `memory.svc` is called during the spawn sequence
(after capability resolution, before `SIGSTART`) to build a memory context block for
insertion into the agent’s initial system prompt. The LLM arrives at the first user
message already aware of preferences, recent history, and pinned facts.

**Injection content (in order):**

1. **User preferences** — `UserPreferenceModel.spec.summary` prose and the corrections
   list. Always injected when a preference model exists.
1. **Recent episodic context** — summaries of the N most recent episodic records
   (default 5, configurable). Gives session continuity without consuming the full window.
1. **Pinned facts** — all semantic records with `pinned: true`, including pinned crew
   records if the agent has crew memory access.

**Injected block (written into system prompt before goalTemplate):**

```
[MEMORY CONTEXT — researcher — injected by memory.svc]

User preferences:
  Alice prefers concise responses in markdown format. Always cite sources. Use
  tables for comparisons — corrected twice. Works America/New_York 09:00-18:00.
  Strong background in distributed systems and Rust. Assumes technical depth;
  dislikes over-explanation.

  Corrections to avoid repeating:
    • "Please use a markdown table for comparisons" (2026-03-20)
    • "Assume I know this — skip the background" (2026-03-18)

Recent session history (last 3):
  • 2026-03-22 [success] Researched quantum computing 2025. 12 sources, 3
    high-confidence topological qubit findings. User approved report without edits.
  • 2026-03-20 [success] Analysed Q3 financials. Found 3 OPEX anomalies, alice
    confirmed all. Report at /users/alice/workspace/q3-report.md.
  • 2026-03-18 [partial] Drafted project-alpha spec. Two user-requested revisions.

Pinned facts:
  • project-alpha-deadline: April 30, 2026 (confirmed 2026-03-22)
  • preferred-template: /users/alice/workspace/templates/report.md
  • [crew:researchers] standard-citation-format: Chicago author-date style
```

-----

## AgentManifest Memory Block (updated)

```yaml
spec:
  memory:
    episodic:
      enabled: true
      autoLogOnSessionEnd: true     # RuntimeExecutor calls memory/log-event at SIGSTOP
      retentionDays: null           # null = use kernel default (30)

    semantic:
      enabled: true
      access: read-write            # none | read-only | read-write

    preferences:
      enabled: true
      autoInjectAtSpawn: true       # memory.svc builds and injects the context block
      autoCaptureCorrections: true  # agent LLM should call memory/update-preference
                                    # when a user correction is observed

    crew:
      readShared: true              # can read crew memory via scope:crew in retrieve
      writeShared: false            # can write to crew memory via scope:crew

    sharing:
      canRequest: false             # can this agent call memory/share-request?
      canReceive: false             # can this agent be targeted by a share-request?
```

-----

## Memory Capability Tokens

|Capability    |Grants                                                                                                        |
|--------------|--------------------------------------------------------------------------------------------------------------|
|`memory:read` |`memory/retrieve`, `memory/get-fact`, `memory/get-preferences`                                                |
|`memory:write`|All `memory:read` tools + `memory/log-event`, `memory/store-fact`, `memory/update-preference`, `memory/forget`|
|`memory:share`|`memory/share-request` — privilege-level; never granted by default; requires explicit operator grant          |

**`llm:inference` and `memory/retrieve`:** `memory.svc` uses the calling agent’s own
`llm:inference` grant (via `_caller` pass-through) for the model re-rank step.
`memory:read` alone is sufficient to call `memory/retrieve` — but without
`llm:inference` in the agent’s token, results are returned at BM25 + vector quality
with no model re-rank and no `relevance` field. Agents that need full retrieval quality
should hold both `memory:read` and `llm:inference`.

**Spawn injection** requires no capability on the agent’s part — `memory.svc` assembles
the context block from stored text using only `fs:read` on the agent’s memory tree.
No inference call is made; no quota is consumed before the agent’s execution loop
begins.

-----

## Memory Sharing — HIL Flow

Memory sharing is the controlled path by which one agent’s private memory records can
be read by a different agent. It always requires human approval via the standard
ApprovalToken / SIGPAUSE / SIGRESUME protocol.

### Constraints

- Requesting agent must hold `memory:share` (`canRequest: true` in manifest).
- Receiving agent must have `canReceive: true` in its manifest.
- Both agents must be owned by the same user (cross-user sharing not supported in v1).
- Grants are read-only — grantee can retrieve shared records but cannot modify or
  delete them.

### Flow

```
Agent A (pid:57, researcher) calls memory/share-request:
  {
    "targetAgent": "writer",
    "recordIds":   ["mem-abc123"],
    "reason":      "Sharing research findings so writer can draft the article",
    "scope":       "session"
  }

memory.svc → kernel:
  ├── Validates: pid 57 holds memory:share ✓
  ├── Validates: "writer" has canReceive: true ✓
  ├── Validates: records belong to pid 57's namespace ✓
  │
  ├── Mints ApprovalToken bound to (pid:57, targetAgent:writer, recordIds:[...], scope)
  ├── Writes /proc/57/hil-queue/hil-004.yaml
  ├── Sends SIGPAUSE to agent 57
  └── Pushes hil.request (type: memory_share)
```

**`hil.request` event body:**

```json
{
  "hilId":       "hil-004",
  "pid":         57,
  "agentName":   "researcher",
  "type":        "memory_share",
  "targetAgent": "writer",
  "records": [
    {
      "id":        "mem-abc123",
      "type":      "episodic",
      "summary":   "Completed web research on quantum computing. Found 12 sources. 3 high-confidence findings on topological qubits.",
      "createdAt": "2026-03-22T14:30:00Z"
    }
  ],
  "reason":        "Sharing research findings so writer can draft the article",
  "scope":         "session",
  "prompt":        "researcher wants to share 1 memory record with writer. Approve?",
  "approvalToken": "appr-mem-3x7q...",
  "expiresAt":     "2026-03-22T14:45:00Z"
}
```

The human sees the full record summary — not an opaque ID. They read exactly what
they are approving before responding.

**On approval:**

```
Kernel:
  ├── Atomically consumes ApprovalToken
  ├── Instructs memory.svc to create MemoryGrant
  ├── Sends SIGRESUME { decision: "approved" } to agent 57
  └── Pushes hil.resolved

memory.svc:
  ├── session-scoped grant → held in-memory; expires when session closes
  └── permanent grant → persisted to
      /users/alice/memory/researcher/grants/<grant-id>.yaml
```

**Using the grant (writer agent):**

Writer calls `memory/retrieve` with `scopes: ["own", "grants"]`. Granted records are
included in results tagged `scope: grant:<grant-id>`. Writer cannot call any write
operation on granted records.

-----

## MemoryGrant Schema

```yaml
apiVersion: avix/v1
kind: MemoryGrant

metadata:
  id: grant-001
  grantedAt: 2026-03-22T14:32:00Z
  grantedBy: alice
  hilId: hil-004

spec:
  grantor:
    agentName: researcher
    owner: alice
  grantee:
    agentName: writer
    owner: alice
  records:
    - mem-abc123
  scope: session
  sessionId: sess-xyz789
  expiresAt: null           # set by memory.svc when the session closes
```

-----

## Isolation Enforcement Summary

|Operation                                      |Enforced by    |Mechanism                                                                  |
|-----------------------------------------------|---------------|---------------------------------------------------------------------------|
|Agent reads own memory                         |`memory.svc`   |Namespace check: caller pid → agentName → owner path                       |
|Agent reads another agent’s memory (no grant)  |`memory.svc`   |`EPERM` — no MemoryGrant found                                             |
|Agent reads another agent’s memory (with grant)|`memory.svc`   |MemoryGrant validated; returned read-only tagged with grant scope          |
|Agent reads crew memory (member)               |`memory.svc`   |Crew membership verified from CapabilityToken; `memory:read` required      |
|Agent reads crew memory (non-member)           |`memory.svc`   |`EPERM` — crew membership check fails                                      |
|Direct `fs/read` on `/users/alice/memory/`     |`memfs.svc` ACL|Agent `fs:read` scope is workspace-only; memory tree is excluded           |
|Cross-user access                              |Structural     |MemoryGrant only supports same-owner in v1                                 |
|Sharing without HIL                            |Kernel         |`memory/share-request` always triggers ApprovalToken flow; no bypass exists|

-----

## Kernel Config — `memory:` stanza (updated)

```yaml
memory:
  defaultContextLimit: 200000

  episodic:
    maxRetentionDays: 30
    maxRecordsPerAgent: 10000

  semantic:
    maxFactsPerAgent: 5000

  retrieval:
    defaultLimit: 5
    maxLimit: 20
    candidateFetchK: 20       # BM25 + vector each return top-K before RRF merge
    rrfK: 60

  spawn:
    episodicContextRecords: 5
    preferencesEnabled: true
    pinnedFactsEnabled: true

  sharing:
    enabled: true
    hilTimeoutSec: 600
    crossUserEnabled: false   # v1: always false
```

-----

## Cron Jobs (updated)

```yaml
jobs:
  - id: memory-gc-daily
    schedule: "0 3 * * *"
    user: svc-memory-gc
    agentTemplate: memory-gc
    goal: >
      Delete episodic records older than retentionDays.
      Prune expired session-scoped MemoryGrants.
      Report: records_deleted, bytes_freed, grants_pruned.
    args:
      retentionDays: 30
    onFailure: alert

  - id: memory-reindex-weekly
    schedule: "0 4 * * 0"
    user: svc-memory-gc
    agentTemplate: memory-reindex
    goal: >
      Rebuild BM25 full-text and vector indexes for all agents.
      For each MemoryRecord where index.vectorModel differs from the current
      configured embedding model, re-embed spec.content and update index.vectorModel.
      Only process the delta — records with a matching vectorModel are skipped.
      Report: records_reindexed, records_skipped, duration_ms.
    onFailure: alert
```

-----

## Runtime State — /proc/services/memory/

```
/proc/services/memory/
├── status.yaml              # service health, total record counts, active grants
├── agents/
│   └── <agent-name>/
│       ├── stats.yaml       # record counts by type, last retrieval, last write
│       └── grants/
│           └── <id>.yaml    # active session-scoped MemoryGrant records
```

-----

## Open Questions

1. **Preference bootstrap for new agents** — a brand new `writer` agent has no
   `UserPreferenceModel` and will give generic output on its first session. Two options:
   (a) auto-copy preferences from a sibling agent owned by the same user; (b) a
   user-level `UserPreferenceModel` at `/users/<username>/memory/preferences.yaml` acts
   as an inherited baseline all agents fall back to. Option (b) is more Unix-like —
   user-level defaults, agent-level overrides — and avoids implicit cross-agent coupling.
1. **Crew memory write policy** — any crew-member agent can currently write to crew
   shared memory without HIL (crew membership is the existing ACL grant). Should crew
   memory writes require a crew-level HIL approval, or is membership sufficient? The
   stricter model is safer; the looser model is better for collaborative workflows.
1. **Cross-user sharing (v2)** — both the owning user and the receiving user would need
   to approve. Requires a two-phase or simultaneous multi-party HIL pattern not yet
   defined in the ATP spec.
1. **Crew-level memory grants (v2)** — a HIL flow that lets a human approve sharing an
   agent’s private memory with all members of a crew in a single approval action.

-----

## Related Documents

- [AgentManifest](./agent-manifest.md) — `spec.memory` block
- [KernelConfig](./kernel-config.md) — `memory:` stanza
- [Capability Token](./capability-token.md) — `memory:read`, `memory:write`, `memory:share`
- [ATP Spec](./atp-spec.md) — HIL flow; `memory_share` is a fourth HIL type alongside `tool_call_approval`, `capability_upgrade`, `escalation`
- [Snapshot](./snapshot.md) — `spec.memory.episodicEvents` and `spec.memory.semanticKeys` counts
- [LLM Service](./llm-service.md) — `llm/complete` used by `memory.svc` for model re-ranking; `llm/embed` for vector index maintenance
- [Crews Spec](./crews.md) — crew membership ACL gating `scope: crew:<n>` operations
- [Crontab](./crontab.md) — `memory-gc-daily`, `memory-reindex-weekly`
- [Filesystem](./filesystem.md) — `/users/<username>/memory/` and `/crews/<crew-name>/memory/` tree layout
