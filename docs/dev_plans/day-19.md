# Day 19 — Tool Registry: Dynamic Registration

> **Goal:** Build the `ToolRegistry` — service to tool owner mapping, runtime add/remove with drain support, tool state management (available/degraded/unavailable), visibility scoping (all/crew/user), and `tool.changed` event emission.

---

## Pre-flight: Verify Day 18

```bash
cargo test --workspace
grep -r "run_until_complete" crates/avix-core/src/
grep -r "max_tool_chain_length" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/lib.rs`: `pub mod tool_registry;`

```
src/tool_registry/
├── mod.rs
├── registry.rs     ← ToolRegistry main struct
├── entry.rs        ← ToolEntry (descriptor, state, visibility, owner)
└── events.rs       ← tool.changed event type
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/tool_registry.rs`:

```rust
use avix_core::tool_registry::{ToolRegistry, ToolEntry, ToolState, ToolVisibility};
use avix_core::types::tool::ToolName;

fn make_entry(name: &str, owner: &str) -> ToolEntry {
    ToolEntry {
        name:       ToolName::parse(name).unwrap(),
        owner:      owner.to_string(),
        state:      ToolState::Available,
        visibility: ToolVisibility::All,
        descriptor: serde_json::json!({"name": name, "description": "test"}),
    }
}

// ── Basic add/remove ──────────────────────────────────────────────────────────

#[tokio::test]
async fn add_and_lookup_tool() {
    let reg = ToolRegistry::new();
    reg.add("github-svc", vec![make_entry("github/list-prs", "github-svc")]).await.unwrap();

    let entry = reg.lookup("github/list-prs").await.unwrap();
    assert_eq!(entry.owner, "github-svc");
    assert_eq!(entry.state, ToolState::Available);
}

#[tokio::test]
async fn lookup_missing_tool_returns_err() {
    let reg = ToolRegistry::new();
    assert!(reg.lookup("ghost/tool").await.is_err());
}

#[tokio::test]
async fn remove_tool() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("svc/tool-a", "svc")]).await.unwrap();
    reg.remove("svc", &["svc/tool-a"], "cleanup", false).await.unwrap();
    assert!(reg.lookup("svc/tool-a").await.is_err());
}

// ── Drain: wait for in-flight calls ──────────────────────────────────────────

#[tokio::test]
async fn remove_with_drain_waits_for_inflight() {
    use std::time::Duration;
    let reg = std::sync::Arc::new(ToolRegistry::new());
    reg.add("svc", vec![make_entry("svc/tool-b", "svc")]).await.unwrap();

    // Simulate an in-flight call
    let guard = reg.acquire("svc/tool-b").await.unwrap();

    let r = std::sync::Arc::clone(&reg);
    let remove_handle = tokio::spawn(async move {
        r.remove("svc", &["svc/tool-b"], "drain test", true).await.unwrap();
    });

    // Release after short delay
    tokio::time::sleep(Duration::from_millis(30)).await;
    drop(guard);

    tokio::time::timeout(Duration::from_millis(200), remove_handle).await
        .expect("drain should complete").unwrap();
    assert!(reg.lookup("svc/tool-b").await.is_err());
}

// ── Tool state management ─────────────────────────────────────────────────────

#[tokio::test]
async fn set_tool_state_degraded() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("svc/tool-c", "svc")]).await.unwrap();
    reg.set_state("svc/tool-c", ToolState::Degraded).await.unwrap();
    let entry = reg.lookup("svc/tool-c").await.unwrap();
    assert_eq!(entry.state, ToolState::Degraded);
}

#[tokio::test]
async fn set_tool_state_unavailable_then_recover() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("svc/tool-d", "svc")]).await.unwrap();
    reg.set_state("svc/tool-d", ToolState::Unavailable).await.unwrap();
    reg.set_state("svc/tool-d", ToolState::Available).await.unwrap();
    assert_eq!(reg.lookup("svc/tool-d").await.unwrap().state, ToolState::Available);
}

// ── Visibility scoping ────────────────────────────────────────────────────────

#[tokio::test]
async fn user_scoped_tool_visible_to_owner() {
    let reg = ToolRegistry::new();
    let mut entry = make_entry("cal/read", "calendar-svc");
    entry.visibility = ToolVisibility::User("alice".into());
    reg.add("calendar-svc", vec![entry]).await.unwrap();

    assert!(reg.lookup_for_user("cal/read", "alice").await.is_ok());
    assert!(reg.lookup_for_user("cal/read", "bob").await.is_err());
}

#[tokio::test]
async fn all_scoped_tool_visible_to_everyone() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("svc/public", "svc")]).await.unwrap();
    assert!(reg.lookup_for_user("svc/public", "alice").await.is_ok());
    assert!(reg.lookup_for_user("svc/public", "bob").await.is_ok());
}

// ── tool.changed event emission ───────────────────────────────────────────────

#[tokio::test]
async fn add_emits_tool_changed_event() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("svc/new-tool", "svc")]).await.unwrap();

    let event = tokio::time::timeout(
        std::time::Duration::from_millis(100), events.recv()
    ).await.unwrap().unwrap();

    assert_eq!(event.op, "added");
    assert!(event.tools.contains(&"svc/new-tool".to_string()));
}

#[tokio::test]
async fn remove_emits_tool_changed_event() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("svc/old", "svc")]).await.unwrap();
    let _ = events.recv().await; // consume add event

    reg.remove("svc", &["svc/old"], "gone", false).await.unwrap();

    let event = tokio::time::timeout(
        std::time::Duration::from_millis(100), events.recv()
    ).await.unwrap().unwrap();
    assert_eq!(event.op, "removed");
}

// ── Count ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn tool_count_accurate() {
    let reg = ToolRegistry::new();
    assert_eq!(reg.tool_count().await, 0);
    reg.add("svc", vec![make_entry("svc/a", "svc"), make_entry("svc/b", "svc")]).await.unwrap();
    assert_eq!(reg.tool_count().await, 2);
}
```

---

## Step 3 — Implement

`ToolRegistry` backed by `Arc<RwLock<HashMap<String, ToolEntry>>>`. `acquire` returns a `ToolCallGuard` (RAII counter using `Arc<Semaphore>`). `remove` with `drain: true` waits on the semaphore. Events broadcast via `tokio::sync::broadcast::channel`.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-19: ToolRegistry — dynamic add/remove, drain, state, visibility, tool.changed"
```

## Success Criteria

- [ ] Add/lookup/remove all work
- [ ] `drain: true` waits for in-flight calls before removing
- [ ] State transitions: available → degraded → available
- [ ] User-scoped tool invisible to other users
- [ ] All-scoped tool visible to everyone
- [ ] `tool.changed` event emitted on add and remove
- [ ] 20+ tests pass, 0 clippy warnings

---
---

