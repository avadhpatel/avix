# Dev Plan: Unified Tool Registry + /tools VFS Mount

**Status**: Phase 3 — Complete (kernel syscalls in registry, /tools VFS mount working)  
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

---

## Goal

Expose all callable tools (kernel syscalls + service tools + Category-2 tools) via a single `/tools/` VFS mount so agents can:
1. **Discover** all available tools (not just granted ones)
2. **Browse** tool descriptors with rwx permissions based on user role
3. **Request access** via HIL path for unavailable tools

---

## Architecture

```
VfsRouter:
├── "/etc/avix"  → LocalProvider
├── "/users"     → LocalProvider
├── "/crews"     → LocalProvider
├── "/services"  → LocalProvider
├── "/tools"     → NEW: ToolRegistryMemFs (in-memory)
└── default:     → MemFs (/proc/, /kernel/)
```

```
/tools/ (ToolRegistryMemFs):
├── kernel/
│   ├── proc/spawn.yaml    (state: available|unavailable, perms: rwx)
│   ├── proc/kill.yaml
│   ├── fs/read.yaml
│   ├── cap/
│   └── ...
├── agent/
│   ├── spawn.yaml
│   └── ...
├── pipe/
├── cap/
├── job/
└── service/
    ├── fs/read.yaml
    └── ...
```

---

## Implementation Phases

### Phase 1: Kernel Syscall Registry

| Task | Description | Files |
|------|-------------|-------|
| 1.1 | Define `SyscallDescriptor` struct | `src/syscall/descriptor.rs` (new) |
| 1.2 | Create `SyscallRegistry` — static list of all kernel syscalls | `src/syscall/registry.rs` (new) |
| 1.3 | Add `kernel/sys/syscall-list` IPC method | `src/kernel/ipc_server.rs` |
| 1.4 | Unit tests for syscall registry | `src/syscall/registry.rs` |

**TDD**: `test_syscall_registry_lists_all_28_syscalls()`

---

### Phase 2: ToolRegistry Unification

| Task | Description | Files |
|------|-------------|-------|
| 2.1 | Add `ToolEntry.capabilities_required` field | `src/tool_registry/entry.rs` |
| 2.2 | Create `ToolRegistry::add_kernel_syscalls()` | `src/tool_registry/registry.rs` |
| 2.3 | Register syscalls at kernel boot | `src/kernel/mod.rs` |
| 2.4 | Verify Category-2 tools still work | Run tests |

**TDD**: `test_tool_registry_contains_kernel_syscalls()`

---

### Phase 3: /tools VFS Mount + Permissions

| Task | Description | Files |
|------|-------------|-------|
| 3.1 | Define `ToolPermissions` struct (owner, crew, all — each rwx) | `src/tool_registry/permissions.rs` (new) |
| 3.2 | Default permissions: `all: r--` (read only) | `src/tool_registry/permissions.rs` |
| 3.3 | Service can override via tool.yaml | `src/tool_registry/scanner.rs` |
| 3.4 | Create `ToolRegistryMemFs` implementing `VfsProvider` | `src/vfs/tools_provider.rs` (new) |
| 3.5 | Implement `read(path)` — returns YAML descriptor | `src/vfs/tools_provider.rs` |
| 3.6 | Implement `list(path)` — returns directory listing | `src/vfs/tools_provider.rs` |
| 3.7 | Render `state` based on caller's user/crew/admin role | `src/vfs/tools_provider.rs` |
| 3.8 | Add `/tools` mount in bootstrap/phase2.rs | `src/bootstrap/phase2.rs` |

**YAML Output** (`/tools/kernel/proc/spawn.yaml`):
```yaml
name: kernel/proc/spawn
description: Spawn a new agent process
short: "Spawn a child agent"
detailed: |
  Permissions: caller must have `agent:spawn` capability
  Args: manifest: AgentManifest
  Returns: { pid: u32, session_id: string }
capabilities_required:
  - agent:spawn
state: available          # or unavailable
permissions:
  owner: rwx              # admin/user who owns
  crew: rw-               # crew members
  all: r--                # everyone (default)
request_access: cap/request-tool  # if unavailable
owner: kernel
```

**Permission Check Logic**:
```
user = caller.username
role = caller.role (admin | user)
tool_perms = tool.permissions

if role == "admin" → return rwx (full access)
if user == tool_perms.owner → return tool_perms.owner perms
if user in caller's.crews && tool_perms.crew is set → return tool_perms.crew perms
return tool_perms.all perms
```

**TDD**: `test_tools_vfs_read_returns_valid_yaml()`, `test_tool_permissions_for_admin()`

---

### Phase 4: Agent Tool Discovery + HIL Path

| Task | Description | Files |
|------|-------------|-------|
| 4.1 | Add `ToolEntry::state_for(token)` method | `src/tool_registry/state.rs` |
| 4.2 | VFS shows `state: unavailable` + `capabilities_required` for non-granted tools | `src/vfs/tools_provider.rs` |
| 4.3 | Include `cap/request-tool` reference in unavailable tool YAML | `src/vfs/tools_provider.rs` |
| 4.4 | Test agent can discover + request access | `src/vfs/tools_provider.rs` |

**Flow**:
```
Agent reads /tools/kernel/proc/kill.yaml
  → sees state: unavailable + capabilities_required: [agent:kill]
  → calls cap/request-tool with appropriate reason
  → HIL approves → tool becomes available
```

**TDD**: `test_unavailable_tool_shows_capability_requirements()`

---

### Phase 5: (Optional) ATP Syscall Domain

| Task | Description | Files |
|------|-------------|-------|
| 5.1 | Add `AtpDomain::Syscall` variant | `src/gateway/atp/types.rs` |
| 5.2 | Add syscall handler | `src/gateway/handlers/syscall.rs` (new) |
| 5.3 | Deprecate old domain handlers | `src/gateway/handlers/` |

**Note**: Skip unless clients need direct ATP access to all tools.

---

## File Changes

| File | Change |
|------|--------|
| `src/syscall/descriptor.rs` | NEW — SyscallDescriptor |
| `src/syscall/registry.rs` | NEW — SyscallRegistry (28+ syscalls) |
| `src/tool_registry/permissions.rs` | NEW — ToolPermissions (rwx model) |
| `src/tool_registry/state.rs` | NEW — per-agent tool state |
| `src/tool_registry/entry.rs` | MOD — add capabilities_required |
| `src/tool_registry/registry.rs` | MOD — add_kernel_syscalls() |
| `src/tool_registry/scanner.rs` | MOD — parse permissions from tool.yaml |
| `src/vfs/tools_provider.rs` | NEW — ToolRegistryMemFs |
| `src/bootstrap/phase2.rs` | MOD — add /tools mount |
| `src/kernel/ipc_server.rs` | MOD — syscall-list IPC |
| `src/gateway/atp/types.rs` | MOD — Syscall domain (optional) |

---

## Success Criteria

| # | Criteria |
|----|----------|
| 1 | `/tools/kernel/proc/spawn.yaml` renders with descriptor |
| 2 | `/tools/agent/spawn.yaml` renders (Category-2) |
| 3 | `/tools/service/fs/read.yaml` renders (service tools) |
| 4 | Admin sees `rwx` on all tools |
| 5 | Non-admin sees permissions based on owner/crew/all |
| 6 | Agent WITHOUT capability sees `state: unavailable` + required capabilities |
| 7 | Agent WITH capability sees `state: available` |
| 8 | All existing tests pass |

---

## Testing Strategy

```bash
# Run all tests
cargo test --workspace

# Coverage target
cargo tarpaulin --workspace --out Html
```