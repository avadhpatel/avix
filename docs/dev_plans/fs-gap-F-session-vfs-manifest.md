# Filesystem Gap F — Session Manifests in `/proc/users/<username>/sessions/`

> **Finding:** Active sessions are stored in `redb` (Day 13) but never reflected in the VFS.
> The spec requires a runtime-visible session manifest at
> `/proc/users/<username>/sessions/<session-id>.yaml` for every active session. This path is
> read-only for agents and other observers — it is kernel-generated ephemeral state.
> The redb store is the source of truth for persistence; the VFS entry is a derived view.
>
> **Scope:** `src/session/store.rs` — when a session is saved, also write (or update) the VFS
> manifest. When a session is deleted, remove the VFS entry. The `SessionStore` receives an
> optional `Arc<MemFs>` handle at construction time.

---

## What the VFS entry looks like

Path: `/proc/users/<username>/sessions/<session-id>.yaml`

```yaml
apiVersion: avix/v1
kind: SessionManifest
metadata:
  sessionId: sess-abc-123
  username: alice
  createdAt: 2026-03-22T10:00:00Z
  updatedAt: 2026-03-22T10:05:30Z
spec:
  agentName: researcher
  goal: "Research Q3 revenue trends"
  status: active       # active | completed | error
  messageCount: 7
```

The `username` field must be stored in `SessionEntry` so the correct VFS path can be
constructed. If `username` is absent (legacy sessions), the VFS write is skipped.

---

## Changes to `SessionEntry`

Add `username: String` field to `SessionEntry` (with `#[serde(default)]` for backward
compatibility with existing redb entries):

```rust
pub struct SessionEntry {
    pub session_id:  String,
    pub username:    String,      // ← add this
    pub agent_name:  String,
    pub goal:        String,
    pub messages:    Vec<serde_json::Value>,
    pub status:      SessionStatus,   // Active | Completed | Error
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    #[default]
    Active,
    Completed,
    Error,
}
```

---

## Step 1 — Write Tests First

Add to `crates/avix-core/tests/session.rs`:

```rust
// ── Finding F: session VFS manifest ──────────────────────────────────────────

fn make_session_for_user(id: &str, username: &str, goal: &str) -> SessionEntry {
    SessionEntry {
        session_id:  id.to_string(),
        username:    username.to_string(),
        agent_name:  "researcher".to_string(),
        goal:        goal.to_string(),
        messages:    vec![],
        status:      SessionStatus::Active,
        created_at:  chrono::Utc::now(),
        updated_at:  chrono::Utc::now(),
    }
}

#[tokio::test]
async fn save_session_writes_vfs_manifest() {
    use avix_core::memfs::{MemFs, VfsPath};
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let entry = make_session_for_user("sess-vfs-01", "alice", "Research goal");
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-01.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/users/alice/sessions/sess-vfs-01.yaml must exist after save"
    );
}

#[tokio::test]
async fn session_vfs_manifest_contains_correct_fields() {
    use avix_core::memfs::{MemFs, VfsPath};
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let entry = make_session_for_user("sess-vfs-02", "bob", "Write a report");
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/bob/sessions/sess-vfs-02.yaml").unwrap();
    let raw = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(raw).unwrap();

    assert!(text.contains("sess-vfs-02"), "manifest must contain session id");
    assert!(text.contains("bob"), "manifest must contain username");
    assert!(text.contains("Write a report"), "manifest must contain goal");
    assert!(text.contains("active") || text.contains("Active"), "manifest must show active status");
    assert!(text.contains("SessionManifest"), "manifest must have kind: SessionManifest");
}

#[tokio::test]
async fn delete_session_removes_vfs_manifest() {
    use avix_core::memfs::{MemFs, VfsPath};
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let entry = make_session_for_user("sess-vfs-del", "alice", "goal");
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-del.yaml").unwrap();
    assert!(vfs.exists(&path).await, "manifest should exist before delete");

    store.delete("sess-vfs-del").await.unwrap();

    assert!(
        !vfs.exists(&path).await,
        "/proc/users/alice/sessions/sess-vfs-del.yaml must be removed after session delete"
    );
}

#[tokio::test]
async fn update_session_updates_vfs_manifest() {
    use avix_core::memfs::{MemFs, VfsPath};
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let mut entry = make_session_for_user("sess-vfs-upd", "alice", "goal");
    store.save(&entry).await.unwrap();

    // Add messages and update status
    entry.messages.push(serde_json::json!({"role": "user", "content": "hello"}));
    entry.messages.push(serde_json::json!({"role": "assistant", "content": "hi"}));
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-upd.yaml").unwrap();
    let raw = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(text.contains("messageCount: 2") || text.contains("2"),
        "manifest must reflect updated message count");
}

#[tokio::test]
async fn save_session_without_vfs_succeeds_silently() {
    // No VFS handle — save must still succeed (VFS write is best-effort)
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db")).await.unwrap();

    let entry = make_session_for_user("sess-novfs", "alice", "goal");
    // Must not panic or error when VFS is not attached
    assert!(store.save(&entry).await.is_ok());
}

#[tokio::test]
async fn multiple_users_sessions_land_in_separate_proc_dirs() {
    use avix_core::memfs::{MemFs, VfsPath};
    use std::sync::Arc;

    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    store.save(&make_session_for_user("s-alice", "alice", "g")).await.unwrap();
    store.save(&make_session_for_user("s-bob", "bob", "g")).await.unwrap();

    assert!(vfs.exists(&VfsPath::parse("/proc/users/alice/sessions/s-alice.yaml").unwrap()).await);
    assert!(vfs.exists(&VfsPath::parse("/proc/users/bob/sessions/s-bob.yaml").unwrap()).await);
    assert!(!vfs.exists(&VfsPath::parse("/proc/users/alice/sessions/s-bob.yaml").unwrap()).await,
        "bob's session must not appear under alice's proc dir");
}
```

---

## Step 2 — Implementation

### 2a. Add `vfs` field and `with_vfs` builder to `SessionStore`

In `src/session/store.rs`:

```rust
pub struct SessionStore {
    db: Arc<redb::Database>,
    vfs: Option<Arc<MemFs>>,
}

impl SessionStore {
    pub async fn open(path: impl AsRef<std::path::Path>) -> Result<Self, AvixError> {
        // ... existing open logic ...
        Ok(Self { db: Arc::new(db), vfs: None })
    }

    pub fn with_vfs(mut self, vfs: Arc<MemFs>) -> Self {
        self.vfs = Some(vfs);
        self
    }
}
```

### 2b. Add `write_vfs_manifest` helper

```rust
impl SessionStore {
    async fn write_vfs_manifest(&self, entry: &SessionEntry) {
        let vfs = match &self.vfs {
            Some(v) => v,
            None => return,
        };
        if entry.username.is_empty() {
            return;
        }
        let message_count = entry.messages.len();
        let manifest = format!(
            "apiVersion: avix/v1\nkind: SessionManifest\nmetadata:\n  sessionId: {id}\n  username: {username}\n  createdAt: {created}\n  updatedAt: {updated}\nspec:\n  agentName: {agent}\n  goal: {goal:?}\n  status: {status}\n  messageCount: {message_count}\n",
            id = entry.session_id,
            username = entry.username,
            created = entry.created_at.to_rfc3339(),
            updated = entry.updated_at.to_rfc3339(),
            agent = entry.agent_name,
            goal = entry.goal,
            status = match entry.status {
                SessionStatus::Active => "active",
                SessionStatus::Completed => "completed",
                SessionStatus::Error => "error",
            },
        );
        let path_str = format!(
            "/proc/users/{}/sessions/{}.yaml",
            entry.username, entry.session_id
        );
        if let Ok(path) = VfsPath::parse(&path_str) {
            let _ = vfs.write(&path, manifest.into_bytes()).await;
        }
    }

    async fn remove_vfs_manifest(&self, session_id: &str, username: &str) {
        let vfs = match &self.vfs {
            Some(v) => v,
            None => return,
        };
        if username.is_empty() {
            return;
        }
        let path_str = format!("/proc/users/{username}/sessions/{session_id}.yaml");
        if let Ok(path) = VfsPath::parse(&path_str) {
            let _ = vfs.delete(&path).await;
        }
    }
}
```

### 2c. Call the helpers in `save` and `delete`

```rust
pub async fn save(&self, entry: &SessionEntry) -> Result<(), AvixError> {
    // ... existing redb write ...
    self.write_vfs_manifest(entry).await;
    Ok(())
}

pub async fn delete(&self, session_id: &str) -> Result<(), AvixError> {
    // Load username before deleting so we can clean up the VFS entry
    let username = self.load(session_id).await?
        .map(|e| e.username)
        .unwrap_or_default();
    // ... existing redb delete ...
    self.remove_vfs_manifest(session_id, &username).await;
    Ok(())
}
```

---

## Step 3 — Verify

```bash
cargo test --workspace
# All 6 new session VFS tests must pass
# Existing session tests (save/load/delete/list) must continue to pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

---

## Success Criteria

- [ ] `SessionStore::with_vfs(vfs)` builder method exists
- [ ] `save()` writes `/proc/users/<username>/sessions/<session-id>.yaml` to VFS when handle is set
- [ ] VFS manifest contains `sessionId`, `username`, `goal`, `agentName`, `status`, `messageCount`
- [ ] `delete()` removes the VFS manifest when session is deleted
- [ ] Updating a session (adding messages) re-writes the manifest with the updated `messageCount`
- [ ] `save()` without VFS handle attached succeeds silently (no error, no panic)
- [ ] Two users' sessions land in separate `/proc/users/<u>/sessions/` directories
- [ ] All existing Day-13 session tests continue to pass
- [ ] 6 new tests pass, 0 clippy warnings
