# workspace-gap-B-write-operations.md

## Overview

Implement Phase 2 of `workspace.svc` — write operations with automatic session history integration. This adds the ability to create and modify files while automatically generating diffs and emitting `PartRecord` entries to the session history.

**What it builds:**
- `workspace/create-project` — create project directories with optional templates
- `workspace/write` — write files with diff generation and history emission
- `workspace/delete` — delete files with history logging
- `_caller` extraction — get user identity from router-injected params

**Depends on:** Gap A (service skeleton) — completed

---

## What to Implement

### Task 1: `_caller` extraction from params

The router injects `_caller` into tool call params for `caller_scoped` services. Extract user identity:

```rust
#[derive(Debug, Deserialize)]
struct CallerInfo {
    user: String,
    pid: u64,
    token: String,
}

fn extract_caller(params: &serde_json::Value) -> String {
    params.get("_caller")
        .and_then(|c| c.get("user"))
        .and_then(|u| u.as_str())
        .unwrap_or("anonymous")
        .to_string()
}
```

### Task 2: `workspace/create-project`

- Input: `project_name` (string), `description?`, `template?` ("python", "web", "empty")
- Creates directory at `/users/<user>/workspace/<project_name>/`
- Optionally creates template files
- Returns: project path, initial manifest
- Emits: `PartRecord` with `FileDiff` for each created file

### Task 3: `workspace/write` with diff generation

- Input: `path` (relative or absolute), `content` (string), `mode?` ("create", "overwrite", "append")
- Reads existing content (if any) for diff
- Writes new content via `kernel/fs/write`
- Generates unified diff between before/after
- Emits `PartRecord { part_type: "file_diff", data: { path, before, after, change_type } }` to session history via `kernel/proc/message-create` and `kernel/proc/part-create`

### Task 4: `workspace/delete`

- Input: `path`, `recursive?` (bool)
- Reads file content before deletion for history
- Deletes via `kernel/fs/delete`
- Emits `PartRecord` with `change_type: Deleted`

### Task 5: Session history integration

Call kernel IPC to emit history records:
- `kernel/proc/message-create` — create a `MessageRecord` for the mutation
- `kernel/proc/part-create` — create `PartRecord` with file diff data

---

## TDD Approach

Write failing tests first.

### Test 1: Caller extraction

```rust
#[test]
fn extract_caller_from_params() {
    let params = serde_json::json!({
        "project": "myapp",
        "_caller": { "user": "alice", "pid": 42, "token": "tok" }
    });
    let user = extract_caller(&params);
    assert_eq!(user, "alice");
}
```

### Test 2: Write emits PartRecord

```rust
#[tokio::test]
async fn workspace_write_creates_file_and_emits_diff() {
    // Write to /users/alice/workspace/test.txt
    // Verify kernel/proc/message-create called with proper payload
    // Verify kernel/proc/part-create called with file_diff data
}
```

### Test 3: Path validation still works

```rust
#[tokio::test]
async fn workspace_write_rejects_outside_workspace() {
    // Try to write to /etc/avix/auth.conf
    // Verify EPERM error
}
```

---

## Detailed Implementation Guidance

### IPC calls for history

```rust
async fn emit_file_diff(
    client: &IpcClient,
    session_id: &Uuid,
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    change_type: &str,
) -> Result<(), WorkspaceError> {
    let msg_id = Uuid::new_v4();
    
    let message = MessageRecord {
        id: msg_id,
        session_id: *session_id,
        sequence: 0, // TODO: get from session
        role: Role::Assistant,
        timestamp: Utc::now(),
        content: format!("File {} {}", path, change_type),
        tokens: None,
    };
    
    let msg_resp = client.call(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "kernel/proc/message-create".into(),
        params: serde_json::json!({ "message": message }),
    }).await?;
    
    let part = PartRecord::file_diff(
        msg_id,
        0,
        path,
        before,
        after,
    );
    
    let part_resp = client.call(JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "2".into(),
        method: "kernel/proc/part-create".into(),
        params: serde_json::json!({ "part": part }),
    }).await?;
    
    Ok(())
}
```

### Tool descriptors

Create `services/workspace/tools/workspace-write.tool.yaml`:
```yaml
name:        workspace/write
description: Write file to workspace with automatic history logging
ipc:
  method:    workspace.write
input:
  path: { type: string, required: true }
  content: { type: string, required: true }
  mode: { type: string, required: false }  # create | overwrite | append
  session_id: { type: string, required: false }
output:
  path: { type: string }
  bytes_written: { type: integer }
```

---

## Acceptance Criteria

- [ ] `_caller` properly extracted from params
- [ ] `workspace/create-project` creates directory and optional template files
- [ ] `workspace/write` writes file and generates diff
- [ ] `workspace/write` emits `PartRecord` to session history
- [ ] `workspace/delete` deletes file and logs deletion to history
- [ ] Paths outside workspace still rejected
- [ ] Tool descriptors registered at startup
- [ ] All tests pass, clippy clean, format clean