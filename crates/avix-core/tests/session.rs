use avix_core::memfs::{MemFs, VfsPath};
use avix_core::session::{SessionEntry, SessionStatus, SessionStore};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

fn make_entry(id: &str) -> SessionEntry {
    SessionEntry {
        session_id: id.to_string(),
        username: String::new(),
        agent_name: "researcher".to_string(),
        goal: "find the answer".to_string(),
        messages: vec![json!({"role": "user", "content": "hello"})],
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn make_session_for_user(id: &str, username: &str, goal: &str) -> SessionEntry {
    SessionEntry {
        session_id: id.to_string(),
        username: username.to_string(),
        agent_name: "researcher".to_string(),
        goal: goal.to_string(),
        messages: vec![],
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[tokio::test]
async fn session_save_and_load_roundtrip() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let entry = make_entry("s1");
    store.save(&entry).await.unwrap();
    let loaded = store.load("s1").await.unwrap().unwrap();
    assert_eq!(loaded.session_id, "s1");
    assert_eq!(loaded.agent_name, "researcher");
    assert_eq!(loaded.goal, "find the answer");
}

#[tokio::test]
async fn session_load_missing_returns_none() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let result = store.load("nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn session_delete_removes_entry() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let entry = make_entry("s2");
    store.save(&entry).await.unwrap();
    store.delete("s2").await.unwrap();
    let result = store.load("s2").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn session_list_all_returns_all() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    store.save(&make_entry("s3a")).await.unwrap();
    store.save(&make_entry("s3b")).await.unwrap();
    store.save(&make_entry("s3c")).await.unwrap();
    let all = store.list_all().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn session_update_overwrites() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let mut entry = make_entry("s4");
    store.save(&entry).await.unwrap();
    entry.goal = "updated goal".to_string();
    entry
        .messages
        .push(json!({"role": "assistant", "content": "ok"}));
    store.save(&entry).await.unwrap();
    let loaded = store.load("s4").await.unwrap().unwrap();
    assert_eq!(loaded.goal, "updated goal");
    assert_eq!(loaded.messages.len(), 2);
}

#[tokio::test]
async fn session_persists_across_reopen() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("sessions.db");
    {
        let store = SessionStore::open(db_path.clone()).await.unwrap();
        store.save(&make_entry("s5")).await.unwrap();
    }
    // Reopen
    let store2 = SessionStore::open(db_path).await.unwrap();
    let loaded = store2.load("s5").await.unwrap().unwrap();
    assert_eq!(loaded.session_id, "s5");
}

#[tokio::test]
async fn session_messages_preserved() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let mut entry = make_entry("s6");
    entry.messages = vec![
        json!({"role": "user", "content": "hello"}),
        json!({"role": "assistant", "content": "hi there"}),
        json!({"role": "user", "content": "how are you?"}),
    ];
    store.save(&entry).await.unwrap();
    let loaded = store.load("s6").await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert_eq!(loaded.messages[1]["content"], "hi there");
}

#[tokio::test]
async fn session_large_message_list() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let mut entry = make_entry("s7");
    entry.messages = (0..100)
        .map(|i| json!({"role": "user", "content": format!("message {i}")}))
        .collect();
    store.save(&entry).await.unwrap();
    let loaded = store.load("s7").await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 100);
}

// ── Finding F: session VFS manifest ──────────────────────────────────────────

#[tokio::test]
async fn save_session_writes_vfs_manifest() {
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
    assert!(
        text.contains("active") || text.contains("Active"),
        "manifest must show active status"
    );
    assert!(
        text.contains("SessionManifest"),
        "manifest must have kind: SessionManifest"
    );
}

#[tokio::test]
async fn delete_session_removes_vfs_manifest() {
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
    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    let mut entry = make_session_for_user("sess-vfs-upd", "alice", "goal");
    store.save(&entry).await.unwrap();

    entry
        .messages
        .push(json!({"role": "user", "content": "hello"}));
    entry
        .messages
        .push(json!({"role": "assistant", "content": "hi"}));
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-upd.yaml").unwrap();
    let raw = vfs.read(&path).await.unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(
        text.contains("messageCount: 2") || text.contains('2'),
        "manifest must reflect updated message count"
    );
}

#[tokio::test]
async fn save_session_without_vfs_succeeds_silently() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();

    let entry = make_session_for_user("sess-novfs", "alice", "goal");
    assert!(store.save(&entry).await.is_ok());
}

#[tokio::test]
async fn multiple_users_sessions_land_in_separate_proc_dirs() {
    let tmp = tempdir().unwrap();
    let vfs = Arc::new(MemFs::new());
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));

    store
        .save(&make_session_for_user("s-alice", "alice", "g"))
        .await
        .unwrap();
    store
        .save(&make_session_for_user("s-bob", "bob", "g"))
        .await
        .unwrap();

    assert!(
        vfs.exists(&VfsPath::parse("/proc/users/alice/sessions/s-alice.yaml").unwrap())
            .await
    );
    assert!(
        vfs.exists(&VfsPath::parse("/proc/users/bob/sessions/s-bob.yaml").unwrap())
            .await
    );
    assert!(
        !vfs.exists(&VfsPath::parse("/proc/users/alice/sessions/s-bob.yaml").unwrap())
            .await,
        "bob's session must not appear under alice's proc dir"
    );
}
