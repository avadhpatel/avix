# workspace.svc — High-Level Workspace Abstraction Service

> **Status**: Draft  
> **Author**: Architect Agent  
> **Date**: 2026-04-02  
> **Target Branch**: `main` (after Sessions + History v2)  
> **Related Docs**:  
> - `docs/architecture/01-filesystem.md` (VFS & MemFS)  
> - `docs/architecture/07-services.md` (Service model)  
> - `docs/architecture/14-agent-persistence.md` & history v2 spec

---

## 1. Background and Motivation

Avix provides a powerful Virtual Filesystem (VFS via MemFS) with tools like `fs/read`, `fs/write`, `fs/stat`, `fs/list`, `fs/watch`, etc. under the `/tools/fs/` namespace. These are backed by the core MemFS driver.

However, for real agent workflows — especially the demo agents planned (Universal Tool Explorer, Persistent Workspace Agent, Multi-Agent Orchestrator) — we need a **higher-level, opinionated, and secure workspace abstraction**:

- Agents should have a clear, default "working directory" per project/session.
- Operations should be scoped and auditable (all changes logged with diffs in session history).
- Common patterns like "create project", "write file with template", "list workspace contents", "snapshot project" should be convenient and safe.
- Capability checks must be explicit and granular (e.g., `workspace:read`, `workspace:write`).
- File changes must feed structured `PartRecord`s (file_diff parts) into the Session history automatically.

A dedicated **`workspace.svc`** service provides this layer without polluting the low-level `fs/*` tools or requiring every agent to implement the same boilerplate.

This service acts as the **recommended entry point** for agents interacting with user-owned persistent storage, while still allowing direct `fs/*` calls when needed for advanced use cases.

---

## 2. Goals

- Provide a clean, high-level API for project/workspace management.
- Automatically integrate with Sessions: every write creates a traceable diff in history.
- Enforce per-user, per-session, per-project scoping via capabilities.
- Support the demo agent suite (single-agent file work + multi-agent collaboration on shared files).
- Keep implementation lightweight — mostly a thin wrapper + policy layer on top of existing MemFS/VFS.
- Register tools under `/tools/workspace/*` (consistent with other services).
- Zero breaking changes to existing `fs/*` tools.

---

## 3. Service Architecture

`workspace.svc` follows the standard Avix service pattern:

- **Manifest**: `service.unit` (TOML) defining name, version, endpoint, required capabilities, and tool list.
- **Registration**: On startup, calls `kernel/ipc/register` with tool schemas.
- **Implementation language**: Rust (same as core crates), using `avix-core` for JSON-RPC handling, VFS access, and capability validation.
- **Socket**: `AVIX_SVC_SOCK` (standard).
- **Dependencies**: 
  - MemFS/VFS provider (via kernel or direct `fs` tools internally).
  - Session & history persistence layer (to emit structured parts on changes).
  - Router for tool dispatch.

**Placement in hierarchy**:
- Tools exposed at `/tools/workspace/*`
- Default root: `/users/<caller.user>/workspace/`
- Per-project subdirs: `/users/<caller.user>/workspace/<project_name>/`

---

## 4. Core Tools (API Surface)

All tools accept a `session_id` (optional — falls back to current invocation's session) for history integration.

### High-Level Tools

1. **`workspace/create-project`**
   - Params: `project_name` (string), `description?`, `template?` (e.g., "python", "web", "empty")
   - Creates directory + optional starter files.
   - Returns: project path, initial manifest.
   - Emits: `PartRecord` of type `file_diff` for created files.

2. **`workspace/list`**
   - Params: `project?`, `recursive?` (bool)
   - Lists projects or files inside a project.
   - Returns structured tree with metadata.

3. **`workspace/read`**
   - Params: `path` (relative to project or absolute under workspace)
   - Reads file content + metadata.
   - Integrates with session for context if needed.

4. **`workspace/write`**
   - Params: `path`, `content`, `mode?` ("create" | "overwrite" | "append")
   - Writes file.
   - **Automatically**:
     - Computes diff (if file existed).
     - Stores structured `PartRecord { part_type: "file_diff", data: { before, after, path } }` in the session.
     - Updates session `last_updated` and summary.

5. **`workspace/delete`**
   - Params: `path`, `recursive?`
   - Safe delete with history logging.

6. **`workspace/snapshot`**
   - Params: `project`, `name?`
   - Creates a named snapshot (copy or git-like commit in history).
   - Useful for checkpoints before risky changes.

7. **`workspace/search`**
   - Params: `query`, `project?`
   - Full-text or filename search (leverages redb indexes if added later).

### Utility / Metadata Tools

- **`workspace/info`** — Returns current user workspace root, quota, active projects.
- **`workspace/set-default`** — Sets default project for the current session/invocation.

---

## 5. Data Model & Integration with Sessions/History

### 5.1 PartRecord Extension (History v2)

The `PartRecord` already supports `FileDiff` (from history v2 gap D). Extend the data schema:

```rust
pub enum PartType {
    // ... existing
    FileDiff {
        path: String,
        before: Option<String>,    // Previous content (None = created)
        after: Option<String>,    // New content (None = deleted)
        change_type: ChangeType, // Created | Modified | Deleted | Renamed
    },
    // ...
}

pub enum ChangeType {
    Created,
    Modified,
    Deleted,
    Renamed { from: String },
}
```

### 5.2 Mutation Flow

Every mutating operation (`write`, `delete`, `create-project`, `snapshot`) **must**:
1. Perform the VFS operation (via internal call to MemFS).
2. Generate a structured diff.
3. Insert `MessageRecord` + `PartRecord`(s) into the current `SessionRecord`.
4. Update `SessionRecord.last_updated` and optionally trigger summary refresh.

### 5.3 Snapshot Storage

Snapshots stored under:
- `/users/<user>/workspace/.snapshots/<project>/<snapshot-name>/` — as directory copies
- Or as metadata in redb with reference to deduplicated content

---

## 6. Security & Capabilities

### 6.1 Required Capabilities

Declared in `service.unit`:
```toml
[capabilities]
required = ["workspace:read", "workspace:write", "workspace:manage-projects"]
```

### 6.2 Granular Enforcement

- Kernel checks caller's `CapabilityToken` before routing to `workspace.svc`.
- Service can further scope to specific projects (e.g., via path prefix checks).
- All operations run with the caller's identity (`_caller` injected via router).
- No direct access outside `/users/<caller.user>/workspace/` (hardened in VFS layer).

### 6.3 Path Validation

```rust
fn is_path_in_workspace(path: &str, user: &str) -> bool {
    let workspace_root = format!("/users/{}/workspace", user);
    path.starts_with(&workspace_root) && !path.contains("..")
}
```

---

## 7. Implementation Details

### 7.1 Service Structure

```
services/workspace/
├── Cargo.toml
├── src/
│   ├── main.rs                  // entrypoint, socket listener
│   ├── service.rs               // registration, tool handlers
│   ├── handlers/
│   │   ├── create_project.rs
│   │   ├── write.rs
│   │   ├── delete.rs
│   │   ├── list.rs
│   │   ├── read.rs
│   │   ├── snapshot.rs
│   │   └── search.rs
│   ├── diff.rs                  // unified diff generator
│   └── error.rs                 // error types
├── service.unit                 // TOML manifest
└── tools/                       // tool descriptor YAML files
    ├── workspace-list.tool.yaml
    ├── workspace-read.tool.yaml
    ├── workspace-write.tool.yaml
    └── ...
```

### 7.2 Key Dependencies (from avix-core)

- `avix_core::persistence::SessionStore` — for emitting history records
- `avix_core::vfs::VfsProvider` — or internal fs tool calls via IPC
- `avix_core::ipc::JsonRpcHandler` — for handling tool calls
- `similar` or `diff` crate — for computing diffs

### 7.3 Registration (at startup)

```json
{
  "method": "ipc.register",
  "params": {
    "_token": "svc-token-<uuid>",
    "name": "workspace",
    "endpoint": "/run/avix/services/workspace-<pid>.sock",
    "tools": []
  }
}
```

Tools are scanned from `AVIX_ROOT/services/workspace/tools/*.tool.yaml`.

### 7.4 Error Handling

Standard JSON-RPC errors + Avix-specific codes:
- `-32000`: `ERR_WORKSPACE_PATH` — path outside workspace
- `-32001`: `ERR_WORKSPACE_PROJECT_NOT_FOUND` — project doesn't exist
- `-32002`: `ERR_WORKSPACE_QUOTA` — quota exceeded
- `-32003`: `ERR_WORKSPACE_SNAPSHOT_NOT_FOUND` — snapshot missing

---

## 8. Implementation Roadmap

### Phase 1 — Minimal Viable (3–5 days)

| Gap | What |
|-----|------|
| workspace-gap-A | Service skeleton + registration + basic read tools |
| workspace-gap-B | Write operations + diff generation + history emission |
| workspace-gap-C | Delete, snapshot, search + advanced features |

### Phase 2 — Full (1 week)

- `workspace/set-default`, `workspace/info`
- Quota management (if time permits)
- VFS permission hardening
- Tests + demo agent wiring

### Phase 3 — Polish

- GUI support for workspace browser (future)
- Git integration (future)

---

## 9. Risks & Open Questions

| Risk | Mitigation |
|------|------------|
| Performance: Diff computation on large files | Add size limits (1MB default) or async handling |
| Conflict resolution: Multi-agent writes to same file | Use file locking or optimistic diffs (future) |
| Migration: Existing files in `/users/<user>/workspace/` | Both `fs/*` and `workspace/*` access same VFS |
| Naming: Confirm workspace namespace vs project | "workspace" is clearer for the service |

---

## 10. Acceptance Criteria

- [ ] Service installs cleanly via `avix service install`
- [ ] All tools appear in `proc/tools/list` and are callable via ATP
- [ ] Writes from agents appear as structured `FileDiff` parts in session history
- [ ] Demo agents (especially Persistent Workspace Agent) can complete file-heavy tasks using only `workspace/*` tools
- [ ] Capability enforcement prevents unauthorized access
- [ ] Data survives daemon restart (requires VFS persistence)
- [ ] Full test coverage for all tools

---

## 11. References

- `docs/architecture/07-services.md` — Service model, registration, `service.unit`
- `docs/architecture/01-filesystem.md` — VFS trees and ownership
- `docs/architecture/14-agent-persistence.md` — Session/Invocation persistence
- `docs/dev_plans/history-v2-gap-D-hierarchical-sessions.md` — MessageRecord, PartRecord
