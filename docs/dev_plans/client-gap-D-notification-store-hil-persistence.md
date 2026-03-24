# Client Gap D — Notification Store, HIL State Machine + Persistence

> **Status:** Pending
> **Priority:** High
> **Depends on:** Client gap A (ATP types — `HilRequestBody`, `HilResolvedBody`)
> **Blocks:** Client gaps G, H (TUI/GUI notification UI)
> **Affects:** `crates/avix-client-core/src/notification.rs`,
>   `crates/avix-client-core/src/persistence.rs`

---

## Problem

Both GUI and CLI need a unified notification store that tracks HIL requests, agent exits,
and system alerts, and persists them across restarts. Without this shared layer, each
client would implement its own notification list and HIL state machine independently.

---

## Scope

Implement `NotificationStore` (in-memory, `Arc<Mutex<…>>`) and a `HilStateMachine` for
the pending → resolved/timeout transition. Add `Persistence` helpers to save/load
`notifications.json` and `layout.json` from the platform data directory. No UI code.

---

## What Needs to Be Built

### 1. `notification.rs`

#### `Notification` type

```rust
use crate::atp::types::{HilOutcome, HilRequestBody};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NotificationKind {
    Hil,
    AgentExit,
    SysAlert,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,            // == hil_id for HIL notifications
    pub kind: NotificationKind,
    pub agent_pid: Option<u64>,
    pub session_id: Option<String>,
    pub message: String,
    pub hil: Option<HilState>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilState {
    pub hil_id: String,
    pub approval_token: String,
    pub prompt: String,
    pub timeout_secs: u32,
    pub outcome: Option<HilOutcome>,   // None = pending
}
```

#### `NotificationStore`

```rust
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

pub struct NotificationStore {
    inner: Arc<Mutex<Vec<Notification>>>,
    changed: broadcast::Sender<()>,   // fired on any mutation
}

impl NotificationStore {
    pub fn new() -> Self { … }

    /// Add a new notification. Fires changed signal.
    pub async fn add(&self, n: Notification) { … }

    /// Mark an existing HIL as resolved. No-op if not found or already resolved.
    pub async fn resolve_hil(&self, hil_id: &str, outcome: HilOutcome) { … }

    /// Mark notification as read.
    pub async fn mark_read(&self, id: &str) { … }

    /// Snapshot of all notifications (newest first).
    pub async fn all(&self) -> Vec<Notification> { … }

    /// Unread count.
    pub async fn unread_count(&self) -> usize { … }

    /// Subscribe to change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<()> { … }
}
```

#### `NotificationFactory` — build `Notification` from ATP events

```rust
impl Notification {
    pub fn from_hil_request(body: &HilRequestBody) -> Self { … }
    pub fn from_agent_exit(pid: u64, session_id: &str, reason: Option<&str>) -> Self { … }
    pub fn from_sys_alert(level: &str, message: &str) -> Self { … }
}
```

---

### 2. `persistence.rs`

```rust
use std::path::{Path, PathBuf};
use crate::error::ClientError;
use crate::notification::Notification;
use serde::{Serialize, de::DeserializeOwned};

/// Returns the platform-appropriate app data directory for Avix clients.
/// Falls back to `~/.local/share/avix` on Linux, `~/Library/Application Support/avix`
/// on macOS if the env-based path is unavailable.
pub fn app_data_dir() -> PathBuf { … }

/// Generic JSON load — returns empty Vec / default if file does not exist.
pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<T, ClientError>
where T: Default { … }

/// Generic JSON save (atomic write: write to .tmp, rename).
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ClientError> { … }

/// Convenience wrappers
pub fn notifications_path() -> PathBuf {
    app_data_dir().join("notifications.json")
}

pub fn layout_path() -> PathBuf {
    app_data_dir().join("ui-layout.json")
}

pub fn load_notifications() -> Result<Vec<Notification>, ClientError> {
    load_json::<Vec<Notification>>(&notifications_path())
}

pub fn save_notifications(ns: &[Notification]) -> Result<(), ClientError> {
    save_json(&notifications_path(), ns)
}
```

Note: atomic write = `write to path.with_extension("tmp")` + `std::fs::rename`. This
avoids partial writes on crash.

---

## Tests

All tests use `tempfile::tempdir()` for the data directory.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::{HilOutcome, HilRequestBody};

    // --- NotificationStore ---

    #[tokio::test]
    async fn add_increases_unread_count() {
        let store = NotificationStore::new();
        let n = Notification::from_sys_alert("warn", "disk low");
        store.add(n).await;
        assert_eq!(store.unread_count().await, 1);
    }

    #[tokio::test]
    async fn mark_read_decreases_unread_count() {
        let store = NotificationStore::new();
        let n = Notification::from_sys_alert("warn", "test");
        let id = n.id.clone();
        store.add(n).await;
        store.mark_read(&id).await;
        assert_eq!(store.unread_count().await, 0);
    }

    #[tokio::test]
    async fn resolve_hil_sets_outcome() {
        let store = NotificationStore::new();
        let body = HilRequestBody {
            hil_id: "h1".into(), pid: 5, session_id: "s1".into(),
            approval_token: "tok".into(), prompt: "ok?".into(), timeout_secs: 600,
        };
        let n = Notification::from_hil_request(&body);
        store.add(n).await;
        store.resolve_hil("h1", HilOutcome::Approved).await;
        let all = store.all().await;
        let hil = all[0].hil.as_ref().unwrap();
        assert_eq!(hil.outcome, Some(HilOutcome::Approved));
    }

    #[tokio::test]
    async fn changed_signal_fires_on_add() {
        let store = NotificationStore::new();
        let mut rx = store.subscribe();
        store.add(Notification::from_sys_alert("info", "test")).await;
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn all_returns_newest_first() {
        let store = NotificationStore::new();
        store.add(Notification::from_sys_alert("info", "first")).await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        store.add(Notification::from_sys_alert("info", "second")).await;
        let all = store.all().await;
        assert_eq!(all[0].message, "second");
    }

    // --- Persistence ---

    #[test]
    fn save_and_load_notifications_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notifications.json");
        let n = Notification::from_sys_alert("info", "hello");
        save_json(&path, &vec![n.clone()]).unwrap();
        let loaded: Vec<Notification> = load_json(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].message, "hello");
    }

    #[test]
    fn load_json_returns_default_if_file_missing() {
        let path = std::path::Path::new("/tmp/avix-test-does-not-exist-12345.json");
        let loaded: Vec<Notification> = load_json(path).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn atomic_write_does_not_leave_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        save_json(&path, &vec!["hello"]).unwrap();
        assert!(!dir.path().join("test.json.tmp").exists());
        assert!(path.exists());
    }
}
```

---

## Dependencies to add to `avix-client-core/Cargo.toml`

```toml
chrono = { version = "0.4", features = ["serde"] }
tempfile = { version = "3", optional = true }   # [dev-dependencies] only
```

---

## Success Criteria

- [ ] `NotificationStore` add / resolve / mark_read / unread_count / all work correctly
- [ ] `changed` broadcast fires on every mutation
- [ ] `save_json` / `load_json` atomic roundtrip passes
- [ ] Missing file returns `Default::default()`, not an error
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
