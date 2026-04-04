# Tool Registry Unification - Remaining Items

**Status**: Phase 3 complete, Phase 4+ pending  
**Last Updated**: 2026-04-01

---

## Completed Work

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | ✅ Done | Kernel Syscall Registry - `SyscallDescriptor` + `SyscallRegistry` with 26 syscalls |
| Phase 2 | ✅ Done | ToolRegistry Unification - kernel syscalls in registry, capabilities_required field |
| Phase 3 | ✅ Done | /tools VFS Mount - lazy population, tool YAML descriptors |

---

## Remaining Items (Phase 4+)

### 1. Linux-Style Permission Model (rwx)

**Goal**: Implement owner/crew/all permissions like Linux file system.

**Required Changes**:

| File | Change |
|------|--------|
| `src/tool_registry/permissions.rs` (NEW) | Define `ToolPermissions` struct with owner/crew/all fields (each rwx) |
| `src/tool_registry/entry.rs` | Add `permissions: ToolPermissions` field to `ToolEntry` |
| `src/tool_registry/scanner.rs` | Parse optional `permissions` from tool.yaml |
| `src/memfs/router.rs` | Check permissions in `generate_tool_yaml()` |

**Design**:
```rust
// ToolPermissions in src/tool_registry/permissions.rs
pub struct ToolPermissions {
    pub owner: String,      // user who owns the tool
    pub crew: String,      // crew name (optional)
    pub all: String,       // "r--", "rw-", "rwx" for everyone
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            owner: "admin".to_string(),
            crew: "".to_string(),
            all: "r--".to_string(),  // Default: read-only for all
        }
    }
}
```

**Permission Check Logic**:
```
Input: caller (username, role, crews[]), tool_permissions

if role == "admin" → return "rwx" (full access)
if caller.username == tool_permissions.owner → return owner perms
if caller.crews contains tool_permissions.crew → return crew perms
return tool_permissions.all perms
```

---

### 2. Per-Agent Tool State (available vs unavailable)

**Goal**: Show `state: unavailable` for tools the agent doesn't have capability for.

**Required Changes**:

| File | Change |
|------|--------|
| `src/tool_registry/state.rs` (NEW) | Define `ToolAccessState` enum + method to compute from token |
| `src/memfs/router.rs` | Pass agent context to `generate_tool_yaml()` to compute per-agent state |

**Design**:
```rust
// In src/tool_registry/state.rs
pub enum ToolAccessState {
    Available,    // Agent has the required capability
    Unavailable,  // Agent does NOT have the required capability
}

impl ToolEntry {
    pub fn access_state(&self, token: &CapabilityToken) -> ToolAccessState {
        // Check if all required capabilities are in token
        if self.capabilities_required.iter().all(|c| token.has_capability(c)) {
            ToolAccessState::Available
        } else {
            ToolAccessState::Unavailable
        }
    }
}
```

**VFS Changes**:
- Need to pass agent's username/role/crews to VFS read context
- Tool YAML should show `state: unavailable` + `capabilities_required` when access denied
- This requires changes to how VFS resolves caller identity (probably from ATP token)

---

### 3. HIL Path for Requesting Access

**Goal**: When agent sees unavailable tool, it can request access via `cap/request-tool`.

**Required Changes**:

| File | Change |
|------|--------|
| `src/vfs/tools_provider.rs` or `src/memfs/router.rs` | Add `request_access: cap/request-tool` to unavailable tool YAML |

**YAML Output for Unavailable Tool**:
```yaml
name: kernel/proc/kill
description: Terminate an agent process
capabilities_required:
  - agent:kill
state: unavailable
owner: kernel
request_access: cap/request-tool
# Agent figures out the right reason to request
```

---

## Implementation Order

1. **Phase 4a**: Implement `ToolPermissions` struct + default to all r--
2. **Phase 4b**: Update scanner to read permissions from tool.yaml
3. **Phase 4c**: Add permissions to VFS output
4. **Phase 5a**: Implement per-agent access state from CapabilityToken
5. **Phase 5b**: Wire agent context into VFS reads
6. **Phase 5c**: Add HIL path reference to unavailable tools

---

## Testing Strategy

```bash
# Run specific tests
cargo test --package avix-core tool_registry::permissions
cargo test --package avix-core tool_registry::state
cargo test --package avix-core memfs::router

# Full test
cargo test --package avix-core --lib
```

---

---

## Packaging & Installation — Future Work

### Remote Binary Upload for Local Installs over ATP

**Context**: `proc/package/install-agent` and `proc/package/install-service` accept a source
string (URL, `github:` spec, or server-side file path). All download/extract work happens
server-side. This means `file:///path` installs only work when the client and server share a
filesystem (local dev). A remote client (e.g. Web-UI on a laptop, server on a remote box)
cannot install from a local `.tar.xz` file it holds.

**Goal**: Allow a client to push a local `.tar.xz` binary directly to the kernel over ATP,
so remote installs from local files work.

**Options to evaluate**:
1. **Chunked ATP upload** — new `proc/package/upload-chunk` ATP command that accepts
   base64-encoded byte chunks + a final `proc/package/install-from-upload` that assembles
   and installs. Simple but slow for large service binaries.
2. **Separate HTTP upload endpoint** — `POST /api/v1/package/upload` (multipart) on the
   ATP gateway's HTTP layer. Returns a temp token; `proc/package/install-agent` accepts
   `upload:<token>` as a source. Faster, standard pattern, fits well with the existing
   HTTP login endpoint.

Option 2 is preferred — the HTTP layer already exists for `/auth/login`.

**Affected files** (when implemented):
- `crates/avix-core/src/gateway/` — add multipart upload handler
- `crates/avix-core/src/syscall/domain/pkg_.rs` — handle `upload:<token>` source
- CLI — add `--file <path>` flag to `avix agent install` / `avix service install`
- Web-UI — file picker in Extensions tab "Install URL" form

---

### Polyglot Services (Python, JavaScript, etc.)

**Context**: The current service model assumes Rust binaries. `service.yaml` has a `language`
field that accepts `"rust"` or `"any"`, and `ServiceInstaller` / `ServiceProcess` both assume
a compiled native binary in `bin/`. There is no mechanism to spawn or package services written
in Python, JavaScript/TypeScript, or other interpreted languages.

**Goal**: Define how non-Rust services are packaged, installed, and spawned so that community
developers can write Avix services in their language of choice.

**Questions to resolve**:

1. **Runtime discovery** — how does the kernel find the right interpreter (`python3`, `node`,
   `deno`, etc.)? Options: require it on `PATH`, embed a runtime version spec in `service.yaml`,
   or bundle the runtime inside the package.

2. **Packaging** — for a Python service the `bin/` dir would contain `.py` files (or a wheel),
   not a compiled binary. The `PackageBuilder` and `PackageValidator` need to know what
   constitutes a valid `bin/` for each language.

3. **`service.yaml` changes** — `language` field needs well-defined values beyond `"rust"` /
   `"any"`. Proposed additions: `"python"`, `"javascript"`, `"typescript"`, `"deno"`.
   May also need `runtime_version` (e.g. `">=3.11"`) and `entrypoint` (e.g. `"main.py"`).

4. **`ServiceProcess` spawning** — currently calls the binary path directly. For interpreted
   languages it needs to prepend the interpreter: `python3 bin/main.py` or `node bin/index.js`.

5. **Dependency management** — Python services may have a `requirements.txt`; JS services a
   `package.json`. Should the installer run `pip install` / `npm install` at install time?
   Or should packages be self-contained (vendored dependencies)?

6. **Sandboxing** — interpreted runtimes have different sandboxing considerations than native
   binaries. Needs evaluation.

**Proposed `service.yaml` sketch**:
```yaml
name: my-python-svc
version: "0.1.0"

[service]
language       = "python"
runtime_version = ">=3.11"
entrypoint     = "bin/main.py"
binary         = ""              # empty for interpreted services
```

**Affected areas** (when designed and implemented):
- `service.yaml` schema + parser (`crates/avix-core/src/service/`)
- `ServiceProcess::spawn` — interpreter prefix logic
- `PackageValidator` — language-specific `bin/` validation rules
- `PackageBuilder` — skip executable permission setting for non-native bins
- `ServiceInstaller` — optional dependency installation step
- `avix package new --type service --language python` scaffold

---

## Notes

- Permission model defaults to `all: r--` (everyone can read but not execute)
- Admin role gets full rwx on all tools
- VFS needs caller context to compute per-agent state - this may require changes to how VFS resolves the calling agent's identity
- HIL path uses existing `cap/request-tool` - just need to reference it in YAML