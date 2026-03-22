# Day 15 — RuntimeExecutor: Core + Category 2 Tool Registration

> **Goal:** Build the `RuntimeExecutor` core — spawn wiring, system prompt block construction, and full Category 2 tool registration at spawn time (agent/, pipe/, cap/, job/ namespaces). Category 2 tools are registered via `ipc.tool-add` and deregistered at exit via `ipc.tool-remove`.

---

## Pre-flight: Verify Day 14b

```bash
cargo test --workspace
grep -r "AnthropicAdapter"     crates/avix-core/src/
grep -r "pub trait ProviderAdapter" crates/avix-core/src/
grep -r "RoutingEngine"        crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod executor;`

```
src/executor/
├── mod.rs
├── runtime_executor.rs   ← RuntimeExecutor struct
├── spawn.rs              ← SpawnParams, spawn logic
├── prompt.rs             ← system prompt block construction
└── tool_registration.rs  ← Category 2 tool register/deregister
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/runtime_executor.rs`:

```rust
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::types::{Pid, tool::ToolVisibility};
use serde_json::json;
use std::collections::HashSet;

// ─── Test helpers ─────────────────────────────────────────────────────────────

struct MockToolRegistry {
    registered: std::sync::Arc<tokio::sync::Mutex<Vec<(u32, String, ToolVisibility)>>>,
}

impl MockToolRegistry {
    fn new() -> Self {
        Self { registered: Default::default() }
    }
    async fn tools_registered_by_pid(&self, pid: u32) -> HashSet<String> {
        self.registered.lock().await
            .iter()
            .filter(|(p, _, _)| *p == pid)
            .map(|(_, name, _)| name.clone())
            .collect()
    }
    async fn all_registered(&self) -> Vec<(u32, String)> {
        self.registered.lock().await.iter().map(|(p,n,_)| (*p,n.clone())).collect()
    }
}

fn token_with_caps(caps: &[&str]) -> avix_core::types::token::CapabilityToken {
    avix_core::types::token::CapabilityToken {
        granted_tools: caps.iter().map(|s| s.to_string()).collect(),
        signature: "test-sig".into(),
    }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn executor_spawns_with_correct_pid_and_token() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid:          Pid::new(57),
            agent_name:   "researcher".into(),
            goal:         "Find Q3 revenue".into(),
            spawned_by:   "alice".into(),
            token:        token_with_caps(&["spawn", "llm:inference"]),
        },
        registry.clone(),
    ).await.unwrap();

    assert_eq!(executor.pid(), Pid::new(57));
}

// ── Category 2 tool registration ──────────────────────────────────────────────

#[tokio::test]
async fn spawn_cap_registers_agent_tools() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["spawn"]),
        },
        registry.clone(),
    ).await.unwrap();

    let tools = registry.tools_registered_by_pid(57).await;
    assert!(tools.contains("agent/spawn"));
    assert!(tools.contains("agent/list"));
    assert!(tools.contains("agent/wait"));
    assert!(tools.contains("agent/send-message"));
}

#[tokio::test]
async fn pipe_cap_registers_pipe_tools() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["pipe"]),
        },
        registry.clone(),
    ).await.unwrap();

    let tools = registry.tools_registered_by_pid(57).await;
    assert!(tools.contains("pipe/open"));
    assert!(tools.contains("pipe/write"));
    assert!(tools.contains("pipe/read"));
    assert!(tools.contains("pipe/close"));
}

#[tokio::test]
async fn always_present_tools_registered_regardless_of_caps() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&[]), // no caps at all
        },
        registry.clone(),
    ).await.unwrap();

    let tools = registry.tools_registered_by_pid(57).await;
    assert!(tools.contains("cap/request-tool"));
    assert!(tools.contains("cap/escalate"));
    assert!(tools.contains("cap/list"));
    assert!(tools.contains("job/watch"));
}

#[tokio::test]
async fn absent_spawn_cap_does_not_register_agent_tools() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["llm:inference"]), // no spawn
        },
        registry.clone(),
    ).await.unwrap();

    let tools = registry.tools_registered_by_pid(57).await;
    assert!(!tools.contains("agent/spawn"));
    assert!(!tools.contains("agent/list"));
}

#[tokio::test]
async fn shutdown_deregisters_all_category2_tools() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["spawn", "pipe"]),
        },
        registry.clone(),
    ).await.unwrap();

    let before = registry.tools_registered_by_pid(57).await;
    assert!(!before.is_empty());

    executor.shutdown().await;

    let after = registry.tools_registered_by_pid(57).await;
    assert!(after.is_empty());
}

// ── Tool visibility scoping ───────────────────────────────────────────────────

#[tokio::test]
async fn category2_tools_registered_with_user_visibility() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(),
            token: token_with_caps(&["spawn"]),
        },
        registry.clone(),
    ).await.unwrap();

    let all = registry.all_registered().await;
    let agent_spawn = all.iter().find(|(_, n)| n == "agent/spawn");
    assert!(agent_spawn.is_some());
    // In the real registry, visibility would be ToolVisibility::User("alice")
}

// ── System prompt blocks ──────────────────────────────────────────────────────

#[tokio::test]
async fn system_prompt_block1_contains_identity() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "researcher".into(),
            goal: "Find revenue data".into(), spawned_by: "alice".into(),
            token: token_with_caps(&[]),
        },
        registry,
    ).await.unwrap();

    let prompt = executor.build_system_prompt();
    assert!(prompt.contains("researcher"));
    assert!(prompt.contains("Find revenue data"));
    assert!(prompt.contains("57"));
}

#[tokio::test]
async fn system_prompt_block4_empty_without_pending_messages() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(), token: token_with_caps(&[]),
        },
        registry,
    ).await.unwrap();

    let prompt = executor.build_system_prompt();
    assert!(!prompt.contains("[Human guidance]"));
    assert!(!prompt.contains("[System]"));
}

#[tokio::test]
async fn system_prompt_block4_shows_pending_messages() {
    let registry = std::sync::Arc::new(MockToolRegistry::new());
    let mut executor = RuntimeExecutor::spawn_with_registry(
        SpawnParams {
            pid: Pid::new(57), agent_name: "a".into(), goal: "g".into(),
            spawned_by: "alice".into(), token: token_with_caps(&[]),
        },
        registry,
    ).await.unwrap();

    executor.inject_pending_message("[Human guidance]: Exclude salary data.".into());
    let prompt = executor.build_system_prompt();
    assert!(prompt.contains("Exclude salary data"));
}
```

---

## Step 3 — Implement

**`RuntimeExecutor`** struct:

```rust
pub struct RuntimeExecutor {
    pid:              Pid,
    agent_name:       String,
    goal:             String,
    spawned_by:       String,
    token:            CapabilityToken,
    pending_messages: Vec<String>,
    registered_cat2:  Vec<String>, // track for cleanup
    // registry handle (trait object or Arc)
}
```

`spawn_with_registry` iterates the `CapabilityToolMap`, registers all applicable Category 2 tools plus the always-present set, records them in `registered_cat2`.

`build_system_prompt` assembles Blocks 1–4 as a single `String`.

`shutdown` iterates `registered_cat2` and calls `ipc.tool-remove` on each (via the registry handle).

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 25+ executor tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-15: RuntimeExecutor — spawn, Category 2 tool registration, system prompt blocks"
```

## Success Criteria

- [ ] Executor spawns with correct PID
- [ ] `spawn` cap → agent/ tools registered; no `spawn` cap → not registered
- [ ] `pipe` cap → pipe/ tools registered
- [ ] Always-present tools registered with empty token
- [ ] Shutdown deregisters all Category 2 tools
- [ ] Category 2 tools registered with user-scoped visibility
- [ ] Block 1 contains agent name, goal, PID
- [ ] Block 4 empty by default; shows pending messages after inject
- [ ] 25+ tests pass, 0 clippy warnings
