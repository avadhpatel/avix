# workspace-gap-A-service-skeleton.md

## Overview

Implement Phase 1 of `workspace.svc` — a high-level workspace abstraction service that provides project-centric file operations with automatic session history integration. This gap establishes the service skeleton, basic tools, and VFS integration.

**What it builds:**
- Service skeleton: `service.unit`, registration, IPC listener
- Basic tools: `workspace/list`, `workspace/read`, `workspace/info`
- VFS integration: workspace root resolution, path scoping
- History integration: write operations emit `PartRecord` with `FileDiff`

**Depends on:** History v2 (gaps B, C, D) — already merged

---

## What to Implement

### Task 1: Service structure and `service.unit`

Create the service package structure in `services/workspace/`:

```
services/workspace/
├── Cargo.toml
├── src/
│   ├── main.rs          # entrypoint, socket listener
│   ├── service.rs       # registration, tool handlers
│   └── lib.rs           # shared types
├── service.unit         # TOML manifest
└── tools/               # tool descriptor YAML files
```

Define `service.unit`:
```toml
name    = "workspace"
version = "0.1.0"

[unit]
description = "High-level workspace abstraction with session history integration"
author      = "avix/workspace"
after       = ["memfs.svc", "router.svc"]

[service]
binary         = "/services/workspace/bin/workspace"
language       = "rust"
restart        = "always"
run_as         = "service"

[capabilities]
caller_scoped = true
host_access   = "filesystem"

[tools]
namespace = "/tools/workspace/"
provides  = []  # scan tools/ directory
```

### Task 2: Service registration and IPC listener

Implement `main.rs` that:
1. Reads `AVIX_SVC_TOKEN`, `AVIX_KERNEL_SOCK`, `AVIX_ROUTER_SOCK`, `AVIX_SVC_SOCK` from env
2. Connects to kernel and calls `ipc.register` with tool list
3. Listens on `AVIX_SVC_SOCK` for incoming tool calls
4. Speaks JSON-RPC 2.0 with 4-byte length-prefix framing (same pattern as other services)

### Task 3: Core tool handlers

Implement read-only tools:

1. **`workspace/list`** — list projects or files in a project
   - Input: `project?` (string), `recursive?` (bool)
   - Output: `Vec<FileEntry> { path, is_dir, size, modified }`

2. **`workspace/read`** — read file content
   - Input: `path` (relative to project or absolute)
   - Output: `FileContent { path, content, metadata }`

3. **`workspace/info`** — workspace metadata
   - Output: `WorkspaceInfo { root, projects, default_project }`

### Task 4: VFS integration

- Resolve workspace root: `/users/<caller.user>/workspace/`
- Path scoping: validate all paths stay within user's workspace
- Use existing VFS syscalls (`kernel/fs/read`, `kernel/fs/list`, `kernel/fs/stat`)

### Task 5: History integration skeleton

- Add `session_id` param to all tool calls
- Create helper to emit `PartRecord` on mutations (for Phase 2)

---

## TDD Approach

Write failing tests first, then implement to make them pass.

### Test 1: Service registration

```rust
// crates/avix-core/tests/workspace_service.rs
#[tokio::test]
async fn workspace_service_registers_with_kernel() {
    // Start workspace.svc and verify ipc.register succeeds
    // Verify tools appear in ToolRegistry
}
```

### Test 2: Tool dispatch

```rust
#[tokio::test]
async fn workspace_list_returns_projects() {
    // Call workspace/list via IPC
    // Verify response structure
}
```

### Test 3: Path scoping

```rust
#[tokio::test]
async fn workspace_rejects_path_outside_workspace() {
    // Attempt to read /etc/avix/auth.conf via workspace/read
    // Verify EPERM error
}
```

---

## Detailed Implementation Guidance

### Key crates/files to modify

| Path | What |
|------|------|
| `services/workspace/` (new) | Service binary package |
| `crates/avix-core/src/service/lifecycle.rs` | Ensure `ipc.register` handler accepts new service |
| `crates/avix-core/src/tool_registry/mod.rs` | Tool registration |
| `crates/avix-core/src/kernel/syscall/domain/fs_.rs` | VFS syscalls |

### Service entry point

```rust
// services/workspace/src/main.rs
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("AVIX_SVC_TOKEN")?;
    let kernel_sock = std::env::var("AVIX_KERNEL_SOCK")?;
    let svc_sock = std::env::var("AVIX_SVC_SOCK")?;

    // Connect to kernel and register
    let mut client = IpcClient::connect(&kernel_sock).await?;
    client.call("ipc.register", RegisterParams {
        _token: token,
        name: "workspace",
        endpoint: &svc_sock,
        tools: vec![],  // scanned from tools/*.tool.yaml
    }).await?;

    // Start IPC server listener on svc_sock
    // Handle incoming tool calls
}
```

### Tool handlers

```rust
// services/workspace/src/service.rs
pub async fn handle_workspace_list(
    params: ListParams,
    caller: &CallerInfo,
) -> Result<ListResponse, WorkspaceError> {
    let workspace_root = format!("/users/{}/workspace", caller.user);
    let project_path = params.project
        .map(|p| format!("{}/{}", workspace_root, p))
        .unwrap_or(workspace_root);

    let entries = vfs_list(&project_path, params.recursive).await?;
    Ok(ListResponse { entries })
}
```

### Error types

```rust
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("path outside workspace: {0}")]
    PathOutsideWorkspace(String),
    #[error("project not found: {0}")]
    ProjectNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### Tracing/logging

- `tracing::debug!("workspace/list called by user={}", caller.user)`
- `tracing::info!("workspace operation: {} {}", operation, path)`

---

## Testing Requirements

### Unit tests

- `workspace_list_empty_project` — list empty workspace
- `workspace_list_with_projects` — list multiple projects
- `workspace_read_file` — read existing file
- `workspace_read_missing` — read non-existent file returns ENOTFOUND
- `workspace_path_validation` — paths outside workspace rejected

### Integration tests

- Full service registration flow
- Tool dispatch through router
- VFS integration (real filesystem or mock)

### Edge cases

- Empty workspace (no projects)
- Very long file names
- Unicode in paths
- Concurrent reads

---

## Usability Considerations

After Phase 1 is complete, the Usability Agent should verify:

1. Service installs cleanly via `avix service install`
2. Tools appear in `proc/tools/list` with correct namespace
3. `workspace/info` shows correct workspace root
4. `workspace/list` returns empty array for new users
5. Error messages are human-readable

---

## Estimated Effort & Priority

| Task | Effort | Priority |
|------|--------|----------|
| Task 1: Service structure | 0.5 days | High |
| Task 2: Registration + listener | 1 day | High |
| Task 3: Core read tools | 1 day | High |
| Task 4: VFS integration | 0.5 days | High |
| Task 5: History skeleton | 0.5 days | Medium |
| **Total** | **3.5 days** | |

**Priority:** High — foundational for Phase 2

---

## Completion Checklist

- [ ] `services/workspace/` created with `service.unit`
- [ ] Service registers with kernel via `ipc.register`
- [ ] Tools visible in `/proc/tools/list`
- [ ] `workspace/list` returns project listing
- [ ] `workspace/read` returns file content
- [ ] `workspace/info` returns workspace metadata
- [ ] Path scoping enforced (rejects paths outside workspace)
- [ ] All tests pass (`cargo test --workspace`)
- [ ] Clippy clean (`cargo clippy --workspace -- -D warnings`)
- [ ] Format clean (`cargo fmt --check`)
