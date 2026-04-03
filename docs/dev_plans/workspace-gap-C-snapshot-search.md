# workspace-gap-C-snapshot-search.md

## Overview

Implement Phase 3 of `workspace.svc` — snapshot, search, and utility features.

**What it builds:**
- `workspace/snapshot` — create named snapshots of projects
- `workspace/search` — search files by name or content
- `workspace/set-default` — set default project for session

**Depends on:** Gap B (write operations) — completed

---

## What to Implement

### Task 1: `workspace/snapshot`

- Input: `project` (string), `name?` (string), `description?`
- Creates a snapshot: copies project files to `/users/<user>/workspace/.snapshots/<project>/<timestamp>-<name>/`
- Returns: snapshot path, file count
- History: emits `PartRecord` with snapshot metadata

### Task 2: `workspace/search`

- Input: `query` (string), `project?`, `type?` ("name" | "content")
- Searches files in workspace
- Returns: list of matches with path and line numbers
- Uses VFS list + content grep pattern

### Task 3: `workspace/set-default`

- Input: `project` (string)
- Sets default project for current session
- Stores in session metadata or user defaults

---

## TDD Approach

```rust
#[test]
fn snapshot_path_format() {
    let name = "test-snapshot";
    let path = format_snapshot_path("alice", "myapp", name);
    assert!(path.contains("myapp"));
    assert!(path.contains(".snapshots"));
}
```

---

## Tool Descriptors

```yaml
# workspace-snapshot.tool.yaml
name:        workspace/snapshot
description: Create a named snapshot of a project
input:
  project: { type: string, required: true }
  name: { type: string, required: false }
  description: { type: string, required: false }
output:
  path: { type: string }
  files: { type: integer }
```

```yaml
# workspace-search.tool.yaml
name:        workspace/search
description: Search files in workspace by name or content
input:
  query: { type: string, required: true }
  project: { type: string, required: false }
  search_type: { type: string, required: false }  # name | content
output:
  results: { type: array }
```

```yaml
# workspace-set-default.tool.yaml
name:        workspace/set-default
description: Set default project for current session
input:
  project: { type: string, required: true }
output:
  project: { type: string }
```

---

## Acceptance Criteria

- [ ] `workspace/snapshot` creates snapshot directory with files
- [ ] `workspace/search` finds files by name
- [ ] `workspace/set-default` stores default project
- [ ] All tool descriptors registered
- [ ] Tests pass, clippy clean, format clean