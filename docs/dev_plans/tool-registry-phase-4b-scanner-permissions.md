# Phase 4b — Scanner Parses Permissions from tool.yaml

**Status**: Not started  
**Last Updated**: 2026-04-13

---

## Goal

Wire `ToolPermissions` from `tool.yaml` into `ToolEntry` via `ToolDescriptor`. Currently
all entries use `ToolPermissions::default()` (owner: `"root"`, crew: `""`, all: `"r--"`).
After this change, tool authors can declare explicit permissions in `tool.yaml` and they
will be respected at dispatch time.

## Phase 4c Status

**Already implemented.** `generate_tool_yaml` in `crates/avix-core/src/memfs/router.rs`
already outputs the full `permissions:` block (owner/crew/all) and per-agent `state:
unavailable` logic via `VfsCallerContext`. No changes needed there.

---

## Architecture References

- `docs/architecture/09-runtime-executor-tools.md` — tool exposure model
- `docs/dev_plans/TODO.md` — Phase 4b entry

---

## Files to Change

| # | File | Change |
|---|------|--------|
| 1 | `crates/avix-core/src/tool_registry/descriptor.rs` | Add `permissions: Option<ToolPermissions>` field to `ToolDescriptor` |
| 2 | `crates/avix-core/src/tool_registry/scanner.rs` | Wire `desc.permissions` (or derive from `desc.owner`) into `ToolEntry` |

No other files need changes.

---

## Step-by-Step Implementation

### Step 1 — `descriptor.rs`

Add a `permissions` field to `ToolDescriptor`:

```rust
use crate::tool_registry::permissions::ToolPermissions;

pub struct ToolDescriptor {
    // ... existing fields ...
    pub owner: Option<String>,             // kept for backwards compat
    
    #[serde(default)]
    pub permissions: Option<ToolPermissions>,
}
```

`ToolPermissions` already derives `serde::Deserialize` + `serde::Serialize`. The field is
`Option` so existing `tool.yaml` files that omit `permissions:` keep working.

**Expected `tool.yaml` shape** (new, optional):
```yaml
permissions:
  owner: alice
  crew: ops
  all: "r--"
```

---

### Step 2 — `scanner.rs`

In `scan_as_entries`, after building each `ToolEntry`, apply permissions:

```rust
// Derive permissions: explicit block wins; fall back to owner field; else default.
let permissions = match desc.permissions.clone() {
    Some(p) => p,
    None => {
        let mut p = ToolPermissions::default();
        if let Some(ref o) = desc.owner {
            p.owner = o.clone();
        }
        p
    }
};
let entry = ToolEntry::new(desc).with_permissions(permissions);
```

This means:
- `tool.yaml` with explicit `permissions:` block → used as-is
- `tool.yaml` with only `owner:` field → owner name applied, crew/all stay at default
- `tool.yaml` with neither → `ToolPermissions::default()` (owner=`"root"`, crew=`""`, all=`"r--"`)

---

## Testing Strategy

**File 1 — `descriptor.rs`**:  
Add a unit test that deserialises a YAML string with a `permissions:` block and asserts
the field is populated correctly. Also assert that a YAML without `permissions:` deserialises
to `None`.

```bash
cargo test --package avix-core tool_registry::descriptor
```

**File 2 — `scanner.rs`**:  
Add tests for all three derivation paths:
1. Explicit `permissions:` block in descriptor → used verbatim
2. Only `owner:` in descriptor → owner name propagated, defaults for crew/all
3. Neither field → full `ToolPermissions::default()`

```bash
cargo test --package avix-core tool_registry::scanner
```

---

## Success Criteria

- `cargo check --package avix-core` passes with zero errors
- `cargo clippy --package avix-core -- -D warnings` passes
- All new tests pass
- Existing `tool_registry` tests still pass
