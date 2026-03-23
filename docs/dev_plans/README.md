# Development Plans

This folder is used for active development initiatives.
It may contain markdown files that will be used for dev reviews and coding
tools to help build the right solution.

Consider files in this folder temporary and can be deleted as per dev's needs.

---

## Development Workflow

After each gap plan is implemented and all tests pass:

1. Run the full verification suite:
   ```bash
   cargo test --workspace          # all tests must pass
   cargo clippy --workspace -- -D warnings  # zero warnings
   cargo fmt --check               # zero formatting diff
   ```
2. Commit the completed gap with a descriptive message, e.g.:
   ```bash
   git commit -m "Implement memory-gap-C: service tools and BM25 search"
   ```

One commit per completed gap. Do not batch multiple gaps into a single commit.

---

## Active Plans

### VFS Persistence (prerequisite for memory)

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `fs-gap-E-local-provider.md` | `StorageProvider` trait + `LocalProvider` (disk-backed) + `VfsRouter` replacing `Arc<MemFs>`; Phase 2 bootstrap mounts `/users/`, `/crews/`, `/services/` to disk | **Critical** | — |

Must be completed before any memory gap. Without it, all memory records are lost on every avix restart.

---

### Memory Service Gaps

Implement `memory.svc` — the full agent memory subsystem per `docs/spec/memory-service.md`.

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `memory-gap-A-schema.md` | Core schema types: MemoryRecord, UserPreferenceModel, MemoryGrant; align KernelConfig.memory and AgentManifest memory block to spec | High | — |
| `memory-gap-B-vfs-layout.md` | VFS tree structure: init memory dirs at spawn, block agent direct writes to memory trees, /proc/services/memory/ bootstrap | High | Gap A |
| `memory-gap-C-service-tools.md` | memory.svc service module and all 7 tool handlers; BM25 search; ACL enforcement | High | Gap A, Gap B |
| `memory-gap-D-capability-spawn.md` | Add memory:read/write/share to CapabilityToolMap; spawn injection of memory context into system prompt; SIGSTOP auto-log | High | Gap A, Gap B, Gap C |
| `memory-gap-E-retrieval-model.md` | Full retrieval: vector index (llm/embed), RRF merge, LLM re-rank; graceful degradation; caller token pass-through | Medium | Gap C, Gap D |
| `memory-gap-F-hil-sharing.md` | memory/share-request HIL flow; MemoryGrant creation; grants scope in retrieve; session grant cleanup | Low | Gap A, Gap C, Gap D |
| `memory-gap-G-gc-cron.md` | memory-gc-daily and memory-reindex-weekly cron tasks; wire CronScheduler into kernel boot | Low | Gap C, Gap E |

### Recommended Build Order

```
fs-gap-E  →  memory-gap-A  →  memory-gap-B  →  memory-gap-C  →  memory-gap-D  →  memory-gap-E
                                                                   ↘  memory-gap-F
                                                                   ↘  memory-gap-G
```

---

### AgentManifest Gaps

Implement the `AgentManifest` static descriptor per `docs/spec/agent-manifest.md`.

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `manifest-gap-A-schema.md` | Core schema types: `AgentManifest`, `ManifestEntrypoint`, `ManifestTools`, `ManifestMemory`, `ManifestSnapshot`, `ManifestDefaults`; YAML round-trip; VFS path helpers | High | — |
| `manifest-gap-B-spawn-resolution.md` | `ManifestLoader` (VFS load + signature verify), `ToolGrantResolver` (required/optional × user ACL), `ModelValidator`, `GoalRenderer`, `SpawnResolver`; extend `SpawnParams` | High | Gap A |

### Recommended Build Order

```
manifest-gap-A  →  manifest-gap-B
```

---

### SessionManifest Gaps

Align `SessionEntry` and the VFS manifest it writes to `docs/spec/session-manifest.md`.

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `session-manifest-gap-A-schema.md` | Add missing fields to `SessionEntry`: `uid`, `shell`, `tty`, `workingDirectory`, `agents: Vec<AgentRef>`, `quotaSnapshot`, `lastActivityAt`, `closedAt`, `closedReason`; replace `SessionStatus` with spec-aligned `SessionState`; update VFS manifest output | Medium | — |

### Recommended Build Order

```
session-manifest-gap-A  (standalone)
```

---

### Snapshot Gaps

| File | Description | Priority | Depends On |
|------|-------------|----------|------------|
| `snapshot-gap-A-schema.md` | Align SnapshotFile envelope: apiVersion/kind, SnapshotMetadata, SnapshotSpec with all fields, CapturedBy/Trigger enums, async SnapshotStore | High | — |
| `snapshot-gap-B-capture.md` | Snapshot capture: SIGSAVE handler writes to VFS, checksum computation, snap/save + snap/list + snap/delete syscalls, auto-snapshot task | High | Snapshot Gap A |
| `snapshot-gap-C-restore.md` | Snapshot restore: checksum verify, fresh CapabilityToken, context rebuild, pending request re-issue, pipe SIGPIPE, snap/restore syscall | Medium | Snapshot Gap A, Gap B |

> **Note:** `SnapshotMemory { episodic_events, semantic_keys }` in snapshot-gap-A are
> count fields populated from memory.svc after memory-gap-C lands (see snapshot-gap-B
> `CaptureParams`). No snapshot plan content has been removed — the plans remain valid.
> The `spec.memory.*` counts complement memory.svc; they do not replace it.

### Recommended Build Order

```
snapshot-gap-A  →  snapshot-gap-B  →  snapshot-gap-C
```

---

## Design Notes

### Conversation History vs Memory

`RuntimeExecutor` maintains `conversation_history: Vec<Message>` in-process and passes
it on every `llm/complete` call (stateless LLMs require full context per call). This
in-session history is **ephemeral** — it is never persisted to VFS or stored in
memory.svc. It is discarded when the executor exits.

`memory.svc` is a **separate, complementary layer**:
- **In-session:** `conversation_history` (held by RuntimeExecutor, passed to every LLM call)
- **Cross-session:** memory.svc episodic/semantic/preference records (VFS-persisted,
  injected as a summary block at the next spawn)

At session end (SIGSTOP with `autoLogOnSessionEnd: true`), the executor asks the LLM
to summarise `conversation_history` and writes that summary — not the raw transcript —
via `memory/log-event`. The raw history is then discarded.

This design means agents gain continuity across sessions without unbounded context
growth. The LLM summary is the cross-session artifact; the full transcript is ephemeral.
