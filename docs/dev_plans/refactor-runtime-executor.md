# Refactor RuntimeExecutor: Split into Small Modules (<500 lines)

## Task Summary
Refactor `crates/avix-core/src/executor/runtime_executor.rs` (2949 lines) into focused modules <500 lines each. Use **sub-struct pattern**: group related fields + methods into new structs, RuntimeExecutor holds them and exposes as public API. Preserve all functionality, tests (in-file #[cfg(test)]), invariants.

## Relevant Specs
- docs/architecture/09-runtime-executor-tools.md (tool dispatch, cat2 registration)
- CLAUDE.md (Crate Structure: logic in avix-core; unit tests #[cfg(test)] in-file)
- Rust conventions: no unwrap(?), thiserror, tracing, async tokio::test.

## Sub-struct Pattern (Key Insight)
Instead of extracting functions to standalone modules, create **sub-structs** that encapsulate related state + behavior:

```rust
// 1. Create new struct in e.g., tool_manager.rs
pub struct ToolManager {
    pub tool_list: Vec<serde_json::Value>,
    pub tool_budgets: ToolBudgets,
    pub hil_required_tools: Vec<String>,
    // ... other related fields
}

impl ToolManager {
    pub fn refresh_tool_list(&mut self) { ... }
    pub fn current_tool_list(&self) -> Vec<serde_json::Value> { ... }
    // ... related methods
}

// 2. RuntimeExecutor holds the sub-struct
pub struct RuntimeExecutor {
    pub tool_manager: ToolManager,
    // ... other fields
}

// 3. RuntimeExecutor exposes methods that delegate to sub-struct
impl RuntimeExecutor {
    pub fn current_tool_list(&self) -> Vec<serde_json::Value> {
        self.tool_manager.current_tool_list()
    }
}

// 4. Tests go in the sub-struct's file (tool_manager.rs)
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_foo() { ... }
}
```

## Files Created (Done)
1. `crates/avix-core/src/executor/tool_manager.rs` (445 lines) - ToolManager struct + methods + tests
2. `crates/avix-core/src/executor/tools.rs` (2 lines) - placeholder
3. Updated `crates/avix-core/src/executor/runtime_executor.rs` to use ToolManager
4. Updated `crates/avix-core/src/executor/mod.rs` - added `pub mod tool_manager;`

## Remaining Steps (Memory -> Proc -> Signals -> Status -> Dispatch)

### Step N: Extract [MODULE] to sub-struct

**Identify fields to move**: Find all fields in RuntimeExecutor related to [MODULE] (e.g., memory_svc, memory_context for memory)

**Identify methods to move**: Find all methods in RuntimeExecutor that operate primarily on those fields

**Create new file**: `crates/avix-core/src/executor/[module].rs`

**In new file**:
1. Define struct with grouped fields
2. Implement methods (move code from runtime_executor.rs)
3. Add `#[cfg(test)]` module with tests

**In runtime_executor.rs**:
1. Add field: `pub [module]: [ModuleName]`
2. In spawn(), initialize the sub-struct
3. Add delegating methods: `pub fn foo(&self) -> T { self.[module].foo() }`
4. Update all `self.field` references to `self.[module].field`

**In mod.rs**: Add `pub mod [module];`

**Verify**: `cargo check --package avix-core`

## Implementation Order (One Sub-struct at a Time)
Verify `cargo check --package avix-core` + targeted tests after each.

1. **memory.rs**: MemoryService, memory_context, init_memory_tree, init_memory_context, auto_log_session_end → MemoryManager
2. **proc.rs**: write_status_yaml, init_proc_files, write_resolved_yaml → ProcManager  
3. **signals.rs**: deliver_signal, capture_snapshot, restore, take_interim, signal handling → SignalManager
4. **status.rs**: build_system_prompt, metrics, status fields → StatusManager
5. **dispatch.rs**: dispatch_via_router, dispatch_category2, run_turn_streaming, run_with_client, run_until_complete → DispatchManager
6. **runtime_executor.rs**: Final integration + remaining tests
7. **Final verify**: `cargo clippy --package avix-core -- -Dwarnings`; `cargo fmt`

## Success Criteria
- All files <500 lines (`wc -l`)
- 100% test pass: `cargo test avix_core::executor`
- No regressions: run existing executor tests
- `cargo clippy --package avix-core -- -Dwarnings`; `cargo fmt`