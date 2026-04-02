# Dev Plan: Unified Tool Registry + /tools VFS Mount

**Status**: Phase 3 — Complete  
**Created**: 2026-04-01  
**Last Updated**: 2026-04-01

---

## Problem Statement

| Current State | Gap |
|---------------|-----|
| Kernel syscalls (`kernel/proc/*`, etc.) | Not in ToolRegistry — handled by separate `SyscallHandler` dispatch |
| Service tools (`fs/read`, `llm/complete`) | In ToolRegistry, but no VFS exposure |
| Category-2 tools (`agent/spawn`, `pipe/*`) | In ToolRegistry, separate from kernel |
| `/tools/` path | Does not exist in VFS |
| Tool discovery | Only via `granted_tools` in CapabilityToken |
| Access control | Tools don't have Linux-style rwx permissions |
| **Permissions storage** | Not persisted — no reboot survivability |

---

## Goal

Expose all callable tools (kernel syscalls + service tools + Category-2 tools) via a single `/tools/` VFS mount so agents can:
1. **Discover** all available tools (not just granted ones)
2. **Browse** tool descriptors with rwx permissions based on user role
3. **Request access** via HIL path for unavailable tools

---

## Completed Phases

### Phase 1: Kernel Syscall Registry ✅
- Created `SyscallDescriptor` struct with name, description, capabilities_required, handler_signature
- Created `SyscallRegistry` with 26 kernel syscalls across 6 domains (proc, fs, cap, sys, sched, snap)

### Phase 2: ToolRegistry Unification ✅
- Added `capabilities_required` field to `ToolEntry`
- Added `add_kernel_syscalls()` method to ToolRegistry
- Added `get_all_entries()` method for VFS population
- Updated scanner to read capabilities from tool.yaml
- Kernel syscalls registered at boot in `bootstrap/mod.rs`

### Phase 3: /tools VFS Mount ✅
- Added `mem_mounts` field to VfsRouter for in-memory VFS mounts
- Added lazy population of `/tools/` from tool registry on first read
- Tool YAML descriptors include: name, description, short, detailed, domain, capabilities_required, state, owner, handler_signature

---

## Phase 4: Linux-Style Permission Model (rwx)

### Permissions Storage Design

**Database**: `<root>/kernel/permissions.db` (redb)

```rust
// File: src/tool_registry/permissions.rs
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissions {
    pub owner: String,   // username who owns the tool
    pub crew: String,   // crew name (empty = no crew permission)
    pub all: String,    // "r--", "rw-", or "rwx" for everyone
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            owner: "root".to_string(),
            crew: String::new(),
            all: "r--".to_string(),     // read-only for everyone
        }
    }
}
```

### Permissions Database Schema

```rust
// File: src/tool_registry/permissions_store.rs
use redb::{Database, TableDefinition};

const TOOL_PERMS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("tool_permissions");

pub struct ToolPermissionsStore {
    db: Arc<Mutex<Database>>,
}

impl ToolPermissionsStore {
    pub async fn open(root: &Path) -> Result<Self, AvixError> {
        let db_path = root.join("kernel/permissions.db");
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let db = Database::create(&db_path)?;
        let write_txn = db.begin_write()?;
        write_txn.open_table(TOOL_PERMS_TABLE)?;
        write_txn.commit()?;
        Ok(Self { db: Arc::new(Mutex::new(db)) })
    }

    pub async fn get(&self, tool_name: &str) -> Result<Option<ToolPermissions>, AvixError>
    pub async fn set(&self, tool_name: &str, perms: &ToolPermissions) -> Result<(), AvixError>
    pub async fn list_all(&self) -> Result<Vec<(String, ToolPermissions)>, AvixError>
}
```

### Permission Check Logic

```rust
fn compute_effective_permissions(caller: &VfsCallerContext, tool_perms: &ToolPermissions) -> String {
    if caller.is_admin { return "rwx".to_string(); }
    if caller.username == tool_perms.owner { return tool_perms.all.clone(); }  // owner gets all perms
    if !tool_perms.crew.is_empty() && caller.crews.contains(&tool_perms.crew) { return "rw-".to_string(); }
    tool_perms.all.clone()
}
```

### Admin Detection

From `/etc/avix/users.yaml`:
- User with `uid: 0` is admin
- Or user with `tools: [all]` in their entry

```rust
impl User {
    pub fn is_admin(&self) -> bool {
        self.uid == 0 || self.tools.as_ref().map(|t| t.contains(&"all".to_string())).unwrap_or(false)
    }
}
```

---

## Phase 5: Per-Agent Tool State

### VfsCallerContext Design

```rust
// File: src/vfs/context.rs
use crate::types::token::CapabilityToken;

#[derive(Debug, Clone)]
pub struct VfsCallerContext {
    pub username: String,
    pub crews: Vec<String>,
    pub is_admin: bool,
    pub token: Option<CapabilityToken>,
}

impl VfsCallerContext {
    pub async fn from_token(root: &Path, token: &CapabilityToken) -> Result<Option<Self>, AvixError> {
        let users_path = root.join("etc/users.yaml");
        let users_yaml = tokio::fs::read_to_string(&users_path).await?;
        let users: crate::config::users::UsersConfig = serde_yaml::from_str(&users_yaml)?;
        let issued_to = token.issued_to.as_ref()?;
        let user = users.find_user(&issued_to.username)?;
        Some(Self {
            username: user.username.clone(),
            crews: user.crews.clone(),
            is_admin: user.is_admin(),
            token: Some(token.clone()),
        })
    }
}
```

### VFS Read Signature Change

**Before:**
```rust
pub async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, AvixError>
```

**After:**
```rust
pub async fn read(&self, path: &VfsPath, caller: Option<&VfsCallerContext>) -> Result<Vec<u8>, AvixError>
```

### Per-Agent State Computation

```rust
fn compute_tool_state(entry: &ToolEntry, caller: &VfsCallerContext) -> ToolState {
    let token = match &caller.token {
        Some(t) => t,
        None => return ToolState::Unavailable,
    };
    let has_caps = entry.capabilities_required.iter().all(|cap| token.has_tool(cap));
    if has_caps { ToolState::Available } else { ToolState::Unavailable }
}
```

---

## Phase 6: HIL Path for Requesting Access

### Unavailable Tool YAML Output

```yaml
name: kernel/proc/kill
description: Terminate an agent process
capabilities_required:
  - agent:kill
state: unavailable
owner: kernel
permissions:
  owner: rwx
  crew: rw-
  all: r--
request_access: cap/request-tool
```

---

## Implementation Order

| Step | Task | Files |
|------|------|-------|
| 4.1 | Create `ToolPermissions` struct | `src/tool_registry/permissions.rs` (NEW) |
| 4.2 | Create `ToolPermissionsStore` (redb) | `src/tool_registry/permissions_store.rs` (NEW) |
| 4.3 | Add `permissions` field to `ToolEntry` | `src/tool_registry/entry.rs` |
| 4.4 | Load permissions from DB at boot | `src/bootstrap/mod.rs` |
| 4.5 | Add permissions to VFS YAML output | `src/memfs/router.rs` |
| 5.1 | Create `VfsCallerContext` struct | `src/vfs/context.rs` (NEW) |
| 5.2 | Update `VfsRouter::read()` signature | `src/memfs/router.rs` |
| 5.3 | Pass caller context from kernel IPC | `src/kernel/ipc_server.rs` |
| 5.4 | Compute per-agent state in YAML generation | `src/memfs/router.rs` |
| 6.1 | Add `request_access` to unavailable tools | `src/memfs/router.rs` |

---

## File Changes Summary

| File | Change |
|------|--------|
| `src/tool_registry/permissions.rs` | NEW — `ToolPermissions` struct |
| `src/tool_registry/permissions_store.rs` | NEW — redb-backed `ToolPermissionsStore` |
| `src/tool_registry/entry.rs` | MOD — add `permissions` field |
| `src/tool_registry/scanner.rs` | MOD — parse optional permissions from tool.yaml |
| `src/vfs/context.rs` | NEW — `VfsCallerContext` struct |
| `src/memfs/router.rs` | MOD — caller context in read(), permissions in YAML |
| `src/kernel/ipc_server.rs` | MOD — pass caller context to VFS |
| `src/bootstrap/mod.rs` | MOD — initialize permissions store at boot |
| `src/config/users.rs` | MOD — add `is_admin()` method |

---

## Success Criteria

| # | Criteria |
|----|----------|
| 1 | `/tools/kernel/proc/spawn.yaml` renders with permissions |
| 2 | Permissions survive daemon restart (loaded from DB) |
| 3 | Admin sees `rwx` on all tools |
| 4 | Non-admin sees permissions based on owner/crew/all |
| 5 | Agent WITHOUT capability sees `state: unavailable` + `request_access: cap/request-tool` |
| 6 | Agent WITH capability sees `state: available` |
| 7 | All existing tests pass |

---

## Testing Strategy

```bash
cargo test --workspace
cargo test --package avix-core tool_registry::permissions
cargo test --package avix-core vfs::context
cargo test --package avix-core memfs::router
```