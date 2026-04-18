# Cat1 0a — Token Resolution from Manifest `requestedCapabilities`

**Status**: Complete ✅  
**Created**: 2026-04-18  
**Arch refs**: `docs/architecture/09-runtime-executor-tools.md`, `docs/architecture/15-packaging.md`

---

## Task Summary

`CapabilityToken` in `kernel/proc/agent.rs` is hardcoded with 3 tools
(`agent/spawn`, `llm/complete`, `llm/embed`). Replace this with dynamic
resolution from the spawned agent's manifest `spec.requestedCapabilities`.

**Capability group format**: `<namespace>:<filter>`

The filter is **always a path prefix after the namespace**, not a semantic
keyword. Any non-`*` filter is treated as a sub-path prefix within the
namespace, matching tools whose name starts with `<namespace>/<filter>`.
This handles both flat names (`fs/read`) and arbitrarily nested names
(`workspace/project/list`) uniformly.

Examples:
- `fs:*` → all tools starting with `fs/` (`fs/read`, `fs/write`, …)
- `llm:*` → all tools starting with `llm/` (`llm/complete`, `llm/embed`, …)
- `llm:inference` → tools starting with `llm/inference/` — if none registered, matches nothing (caller used a wrong sub-path; `llm:*` is the correct group for all LLM tools)
- `kernel:*` → all syscalls in SyscallRegistry (every `kernel/*` name)
- `kernel:proc` → syscalls whose `domain == "proc"` (`kernel/proc/spawn`, …)
- `proc:*` → syscalls whose `domain == "proc"` (alias: `proc` domain in SyscallRegistry)
- `workspace:*` → all tools starting with `workspace/`
- `workspace:project` → tools starting with `workspace/project/` (`workspace/project/list`, `workspace/project/create`, …)
- `cap:*` → all tools starting with `cap/`
- `session:*` → all tools starting with `session/`
- `agent:*` → all tools starting with `agent/`

**Fallback**: if `requested_capabilities` is empty, grant only `ALWAYS_PRESENT`
tools (`cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch`). If
ToolRegistry has no tools yet (services not started), resolve what is available
at the time and warn; do not error.

**`ALWAYS_PRESENT` tools are always included** regardless of
`requestedCapabilities`.

---

## Confirmed Features

1. `ManifestScanner::get_manifest(name, username)` — loads the full
   `AgentManifest` (including `spec.requested_capabilities`) for a named agent.
2. `CapabilityResolver` — maps `Vec<String>` (capability groups) to `Vec<String>`
   (granted tool names) by querying ToolRegistry + SyscallRegistry.
3. `AgentManager::spawn()` — uses the resolver instead of the hardcoded token.
4. `AgentManager` receives `Arc<Mutex<Option<Arc<ToolRegistry>>>>` so it sees
   the live registry value set by `ProcHandler::set_tool_registry()`.

---

## Files to Change

| # | File | Action | Rationale |
|---|------|---------|-----------|
| 1 | `crates/avix-core/src/agent_manifest/scanner.rs` | Edit | Add `get_manifest(name, username)` returning `Option<AgentManifest>` |
| 2 | `crates/avix-core/src/kernel/capability_resolver.rs` | **NEW** | `CapabilityResolver` struct + `resolve()` method |
| 3 | `crates/avix-core/src/kernel/mod.rs` | Edit | Add `pub mod capability_resolver;` |
| 4 | `crates/avix-core/src/kernel/proc/agent.rs` | Edit | Add `tool_registry` field; use resolver in `spawn()` |
| 5 | `crates/avix-core/src/kernel/proc/mod.rs` | Edit | Pass shared `tool_registry` Arc to all `AgentManager::new()` rebuild sites |

---

## Implementation Order

### Step 1 — `crates/avix-core/src/agent_manifest/scanner.rs`

Add a new `get_manifest` method that scans system and user `bin/` directories
and returns the first full `AgentManifest` matching `name`:

```rust
/// Load the full AgentManifest for a named agent.
/// Checks system `/bin/` first, then `/users/<username>/bin/`.
/// Returns `None` if no matching manifest is found.
pub async fn get_manifest(&self, name: &str, username: &str) -> Option<AgentManifest> {
    // search /bin/<name>@*/ then /users/<username>/bin/<name>@*/
    // reuse scan_dir logic; return first match where manifest.metadata.name == name
}
```

**Tests** (in `scanner.rs` `#[cfg(test)]` block):
- `get_manifest_returns_full_manifest` — write manifest with `requestedCapabilities`, assert `get_manifest` returns it with the right `spec.requested_capabilities`.
- `get_manifest_returns_none_for_unknown` — assert `None` for an agent not installed.

---

### Step 2 — `crates/avix-core/src/kernel/capability_resolver.rs` (NEW)

```rust
use crate::syscall::SyscallRegistry;
use crate::tool_registry::ToolRegistry;

pub struct CapabilityResolver<'a> {
    tool_registry: &'a ToolRegistry,
    syscall_registry: &'a SyscallRegistry,
}

impl<'a> CapabilityResolver<'a> {
    pub fn new(tool_registry: &'a ToolRegistry, syscall_registry: &'a SyscallRegistry) -> Self { ... }

    /// Map capability group strings to concrete tool names.
    /// `kernel:*` → all SyscallRegistry names.
    /// `<domain>:*` (non-kernel) → SyscallRegistry names where `syscall.domain == domain`.
    /// `<ns>:*` → ToolRegistry tools where name starts with `<ns>/`.
    /// `<ns>:<filter>` → ToolRegistry tools where name starts with `<ns>/` and
    ///                    the verb part contains `<filter>`.
    /// ALWAYS_PRESENT tools are always appended.
    pub async fn resolve(&self, capabilities: &[String]) -> Vec<String> { ... }
}
```

**Matching rules (in priority order)**:
1. Parse `<ns>:<filter>` (split on first `:`; treat missing `:` as `<ns>:*`).
2. **SyscallRegistry** — if `ns == "kernel"`:
   - `filter == "*"` → include all syscalls.
   - `filter != "*"` → include syscalls whose `domain == filter`.
   Otherwise (`ns != "kernel"`): include syscalls whose `domain == ns`
   and (if `filter != "*"`) whose name starts with `kernel/<ns>/<filter>`
   (e.g. `proc:spawn` → `kernel/proc/spawn`).
3. **ToolRegistry** — build a prefix string:
   - `filter == "*"` → prefix = `"<ns>/"`
   - `filter != "*"` → prefix = `"<ns>/<filter>"`
   Include any tool whose name starts with that prefix. This handles
   both flat (`fs/read`) and nested (`workspace/project/list`) tool names.
4. Append `ALWAYS_PRESENT` constants (`cap/request-tool`, `cap/escalate`, `cap/list`, `job/watch`).
5. Deduplicate and sort.

**Tests** (in `capability_resolver.rs` `#[cfg(test)]` block):
- `resolve_fs_star_matches_fs_tools` — populate ToolRegistry with `fs/read`, `fs/write`; assert both returned for `fs:*`.
- `resolve_kernel_star_matches_all_syscalls` — assert all SyscallRegistry names returned for `kernel:*`.
- `resolve_kernel_proc_filter_matches_proc_domain` — assert only `kernel/proc/*` syscalls returned for `kernel:proc`.
- `resolve_proc_star_matches_proc_domain` — assert same `kernel/proc/*` syscalls returned for `proc:*`.
- `resolve_nested_tool_names` — populate ToolRegistry with `workspace/project/list`, `workspace/project/create`, `workspace/create-project`; assert `workspace:project` returns only the first two (prefix `workspace/project`), and `workspace:*` returns all three.
- `resolve_empty_capabilities_returns_always_present_only` — assert only 4 always-present tools.
- `resolve_deduplicates_tools` — provide overlapping groups; assert no duplicates.
- `always_present_tools_always_included` — assert ALWAYS_PRESENT tools appear even when not in capability list.

---

### Step 3 — `crates/avix-core/src/kernel/mod.rs`

Add:
```rust
pub mod capability_resolver;
```

No tests needed for this file (module declaration only).

---

### Step 4 — `crates/avix-core/src/kernel/proc/agent.rs`

**Struct changes**:
```rust
pub struct AgentManager {
    // existing fields...
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,  // NEW
}
```

**`new()` signature** — add one parameter:
```rust
pub fn new(
    ...,  // existing params
    tool_registry: Arc<Mutex<Option<Arc<ToolRegistry>>>>,  // NEW (last param)
) -> Self
```

**`spawn()` changes** — replace hardcoded token block:

```rust
// Resolve granted_tools from manifest requestedCapabilities
let granted_tools = self.resolve_granted_tools(name, caller_identity).await;

let token = CapabilityToken::mint(
    granted_tools,
    Some(issued_to),
    3600,
    &self.master_key,
);
```

Add a private helper:
```rust
async fn resolve_granted_tools(&self, agent_name: &str, caller: &str) -> Vec<String> {
    // 1. Load manifest from ManifestScanner
    let cap_groups: Vec<String> = if let Some(scanner) = &self.manifest_scanner {
        match scanner.get_manifest(agent_name, caller).await {
            Some(m) => m.spec.requested_capabilities,
            None => {
                warn!(agent_name, "manifest not found; granting minimal tools");
                vec![]
            }
        }
    } else {
        warn!("manifest_scanner not wired; granting minimal tools");
        vec![]
    };

    // 2. Resolve capability groups → tool names
    let syscall_reg = SyscallRegistry::new();
    if let Some(tool_reg) = self.tool_registry.lock().await.as_ref() {
        let resolver = CapabilityResolver::new(tool_reg, &syscall_reg);
        resolver.resolve(&cap_groups).await
    } else {
        // ToolRegistry not yet populated — fall back to always-present + minimal set
        warn!(agent_name, "tool_registry not wired; granting always-present tools only");
        ALWAYS_PRESENT.iter().map(|s| s.to_string()).collect()
    }
}
```

**Tests** (in `agent.rs` `#[cfg(test)]` block):
- `spawn_resolves_tools_from_manifest` — create a ManifestScanner backed by a VFS with a manifest that has `requestedCapabilities: [fs:*, llm:*]`; populate a ToolRegistry with `fs/read`, `llm/complete`; assert the spawned agent's token contains those tools.
- `spawn_falls_back_when_manifest_missing` — no manifest in scanner; assert spawned token contains only ALWAYS_PRESENT tools.
- `spawn_falls_back_when_registry_empty` — manifest present but no ToolRegistry; assert ALWAYS_PRESENT tools only.

---

### Step 5 — `crates/avix-core/src/kernel/proc/mod.rs`

Pass the shared `tool_registry` Arc to all `AgentManager::new()` call sites (5 rebuild sites identified: `ProcHandler::new()`, `new_with_factory()`, `with_invocation_store()`, `with_manifest_scanner()`, `with_session_store()`).

Since `ProcHandler.tool_registry` is already `Arc<Mutex<Option<Arc<ToolRegistry>>>>`, pass `Arc::clone(&self.tool_registry)` to each `AgentManager::new()` call. This means `AgentManager` automatically sees the value set by `ProcHandler::set_tool_registry()` without any rebuild.

No new tests needed here — coverage comes from Step 4 tests.

---

## Testing Strategy

```bash
# Step 1
cargo test --package avix-core agent_manifest::scanner

# Step 2
cargo test --package avix-core kernel::capability_resolver

# Step 4
cargo test --package avix-core kernel::proc::agent

# Compile check after each step
cargo check --package avix-core
```

Target: 95%+ coverage of all new/modified functions.

---

## Architecture Notes

- `SyscallRegistry::new()` is cheap (static list); constructing it per-spawn is fine.
- `CapabilityResolver` is stateless and borrows both registries; no Arc needed.
- The `ALWAYS_PRESENT` constant (`["cap/request-tool", "cap/escalate", "cap/list", "job/watch"]`) lives in `crates/avix-core/src/router/capability.rs` and is re-exported via `crates/avix-core/src/router/mod.rs`.
- If `requested_capabilities` contains a group that matches nothing (e.g., a typo), the resolver silently skips it and emits a `tracing::debug!`. This is intentional — manifests may declare aspirational capabilities that aren't yet installed.
