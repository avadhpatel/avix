# Day 13 — Session Storage with redb

> **Goal:** Implement persistent session storage using `redb` (embedded key-value store). Sessions survive process restarts. Agent conversation history is stored and retrieved per session ID. Target: <1ms read/write for typical session sizes.

---

## Pre-flight: Verify Day 12

```bash
cargo test --workspace
grep -r "fn bootstrap_with_root" crates/avix-core/src/
grep -r "has_master_key"         crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Add redb Dependency

In `crates/avix-core/Cargo.toml`:

```toml
[dependencies]
redb = "2"
```

Add to `src/lib.rs`: `pub mod session;`

```
src/session/
├── mod.rs
├── store.rs     ← SessionStore backed by redb
└── entry.rs     ← SessionEntry (conversation history + metadata)
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/session.rs`:

```rust
use avix_core::session::{SessionStore, SessionEntry};
use tempfile::tempdir;

fn make_session(id: &str, goal: &str) -> SessionEntry {
    SessionEntry {
        session_id:  id.to_string(),
        agent_name:  "researcher".to_string(),
        goal:        goal.to_string(),
        messages:    vec![],
        created_at:  chrono::Utc::now(),
        updated_at:  chrono::Utc::now(),
    }
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn save_and_load_session() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();

    let entry = make_session("sess-001", "Research Q3 revenue");
    store.save(&entry).await.unwrap();

    let loaded = store.load("sess-001").await.unwrap();
    assert_eq!(loaded.goal, "Research Q3 revenue");
}

#[tokio::test]
async fn load_missing_session_returns_none() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();
    assert!(store.load("ghost-id").await.unwrap().is_none());
}

#[tokio::test]
async fn update_session_messages() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();

    let mut entry = make_session("sess-002", "Write report");
    store.save(&entry).await.unwrap();

    entry.messages.push(serde_json::json!({"role": "user", "content": "start"}));
    store.save(&entry).await.unwrap();

    let loaded = store.load("sess-002").await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 1);
}

#[tokio::test]
async fn delete_session() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();

    store.save(&make_session("sess-del", "goal")).await.unwrap();
    store.delete("sess-del").await.unwrap();
    assert!(store.load("sess-del").await.unwrap().is_none());
}

// ── Persistence across restarts ───────────────────────────────────────────────

#[tokio::test]
async fn session_survives_store_close_and_reopen() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("sessions.db");

    {
        let store = SessionStore::open(db_path.clone()).await.unwrap();
        store.save(&make_session("persist-01", "Survive restart")).await.unwrap();
        // Drop closes the store
    }

    // Reopen
    let store2 = SessionStore::open(db_path).await.unwrap();
    let loaded = store2.load("persist-01").await.unwrap().unwrap();
    assert_eq!(loaded.goal, "Survive restart");
}

// ── List sessions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_all_sessions() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();

    for i in 0..5u32 {
        store.save(&make_session(&format!("sess-{i}"), "goal")).await.unwrap();
    }

    let all = store.list_all().await.unwrap();
    assert_eq!(all.len(), 5);
}
```

---

## Step 3 — Implement

`SessionStore` wraps a `redb::Database`. `SessionEntry` serialises to/from JSON. Use a single `TABLE: TableDefinition<&str, &str>`.

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
git commit -m "day-13: SessionStore with redb — persistence, CRUD, restart survival"
```

## Success Criteria

- [ ] Save/load round-trip preserves all fields
- [ ] Session survives store close and reopen
- [ ] Missing session returns `None` (not an error)
- [ ] Delete removes session
- [ ] List returns correct count
- [ ] 12+ tests pass, 0 clippy warnings

---
---

