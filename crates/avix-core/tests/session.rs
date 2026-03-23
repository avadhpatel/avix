use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::session::{AgentRole, QuotaSnapshot, SessionEntry, SessionState, SessionStore};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_entry(id: &str) -> SessionEntry {
    let quota = QuotaSnapshot {
        tokens_limit: 500_000,
        agents_limit: 5,
        ..Default::default()
    };
    let mut entry = SessionEntry::new(id.to_string(), String::new(), 0, quota);
    entry.agent_name = "researcher".to_string();
    entry.goal = "find the answer".to_string();
    entry.messages = vec![json!({"role": "user", "content": "hello"})];
    entry
}

fn make_session_for_user(id: &str, username: &str, goal: &str) -> SessionEntry {
    let quota = QuotaSnapshot {
        tokens_limit: 500_000,
        agents_limit: 5,
        ..Default::default()
    };
    let mut entry = SessionEntry::new(id.to_string(), username.to_string(), 0, quota);
    entry.goal = goal.to_string();
    entry
}

async fn store_with_vfs(dir: &std::path::Path) -> (SessionStore, Arc<VfsRouter>) {
    let vfs = Arc::new(VfsRouter::new());
    let store = SessionStore::open(dir.join("sessions.db"))
        .await
        .unwrap()
        .with_vfs(Arc::clone(&vfs));
    (store, vfs)
}

// ── Basic CRUD ────────────────────────────────────────────────────────────────

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

// ── SessionEntry struct behaviour ─────────────────────────────────────────────

// T-SMA-05
#[tokio::test]
async fn session_entry_round_trips_redb() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let mut entry = SessionEntry::new(
        "sess-rt".into(),
        "alice".into(),
        1001,
        QuotaSnapshot {
            tokens_limit: 500_000,
            agents_limit: 5,
            ..Default::default()
        },
    );
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    store.save(&entry).await.unwrap();
    let loaded = store.load("sess-rt").await.unwrap().unwrap();
    assert_eq!(loaded.agents.len(), 1);
    assert_eq!(loaded.agents[0].pid, 57);
    assert_eq!(loaded.quota_snapshot.tokens_limit, 500_000);
    assert_eq!(loaded.state, SessionState::Active);
}

#[tokio::test]
async fn session_closed_state_persists() {
    let tmp = tempdir().unwrap();
    let store = SessionStore::open(tmp.path().join("sessions.db"))
        .await
        .unwrap();
    let mut entry = make_session_for_user("sess-close", "alice", "goal");
    entry.close("test close");
    store.save(&entry).await.unwrap();
    let loaded = store.load("sess-close").await.unwrap().unwrap();
    assert_eq!(loaded.state, SessionState::Closed);
    assert_eq!(loaded.closed_reason.as_deref(), Some("test close"));
    assert!(loaded.closed_at.is_some());
}

// ── VFS manifest ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn save_session_writes_vfs_manifest() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let entry = make_session_for_user("sess-vfs-01", "alice", "Research goal");
    store.save(&entry).await.unwrap();
    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-01.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "/proc/users/alice/sessions/sess-vfs-01.yaml must exist after save"
    );
}

// T-SMA-06
#[tokio::test]
async fn vfs_manifest_matches_spec_schema() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let mut entry = SessionEntry::new(
        "sess-vfs-02".into(),
        "bob".into(),
        1001,
        QuotaSnapshot {
            tokens_used: 0,
            tokens_limit: 500_000,
            agents_running: 1,
            agents_limit: 5,
        },
    );
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    store.save(&entry).await.unwrap();

    let path = VfsPath::parse("/proc/users/bob/sessions/sess-vfs-02.yaml").unwrap();
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();

    assert!(content.contains("kind: SessionManifest"), "missing kind");
    assert!(content.contains("shell:"), "missing shell");
    assert!(content.contains("tty:"), "missing tty");
    assert!(
        content.contains("workingDirectory:"),
        "missing workingDirectory"
    );
    assert!(content.contains("pid: 57"), "missing agent pid");
    assert!(content.contains("role: primary"), "missing agent role");
    assert!(content.contains("tokensLimit:"), "missing tokensLimit");
    assert!(content.contains("agentsLimit:"), "missing agentsLimit");
    assert!(content.contains("state: active"), "missing state");
    assert!(
        content.contains("lastActivityAt:"),
        "missing lastActivityAt"
    );
    // messages must NOT appear in VFS manifest
    assert!(
        !content.contains("messages:"),
        "messages must not be in VFS manifest"
    );
}

#[tokio::test]
async fn session_vfs_manifest_contains_session_id_and_user() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let entry = make_session_for_user("sess-vfs-03", "carol", "Write a report");
    store.save(&entry).await.unwrap();
    let path = VfsPath::parse("/proc/users/carol/sessions/sess-vfs-03.yaml").unwrap();
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();
    assert!(
        content.contains("sess-vfs-03"),
        "manifest must contain session id"
    );
    assert!(content.contains("carol"), "manifest must contain username");
    assert!(
        content.contains("SessionManifest"),
        "manifest must have kind: SessionManifest"
    );
}

#[tokio::test]
async fn delete_session_removes_vfs_manifest() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let entry = make_session_for_user("sess-vfs-del", "alice", "goal");
    store.save(&entry).await.unwrap();
    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-del.yaml").unwrap();
    assert!(
        vfs.exists(&path).await,
        "manifest should exist before delete"
    );
    store.delete("sess-vfs-del").await.unwrap();
    assert!(
        !vfs.exists(&path).await,
        "manifest must be removed after session delete"
    );
}

// T-SMA-07
#[tokio::test]
async fn vfs_manifest_updated_on_close() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let mut entry = make_session_for_user("sess-vfs-cls", "alice", "goal");
    store.save(&entry).await.unwrap();
    entry.close("test close");
    store.save(&entry).await.unwrap();
    let path = VfsPath::parse("/proc/users/alice/sessions/sess-vfs-cls.yaml").unwrap();
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();
    assert!(content.contains("state: closed"), "must show closed state");
    assert!(content.contains("closedAt:"), "must have closedAt");
    assert!(content.contains("test close"), "must have closedReason");
}

// T-SMA-08
#[tokio::test]
async fn multiple_agents_in_vfs_manifest() {
    let tmp = tempdir().unwrap();
    let (store, vfs) = store_with_vfs(tmp.path()).await;
    let mut entry = make_session_for_user("sess-multi", "alice", "goal");
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    entry.add_agent(58, "writer".into(), AgentRole::Subordinate);
    store.save(&entry).await.unwrap();
    let path = VfsPath::parse("/proc/users/alice/sessions/sess-multi.yaml").unwrap();
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();
    assert!(content.contains("pid: 57"));
    assert!(content.contains("role: primary"));
    assert!(content.contains("pid: 58"));
    assert!(content.contains("role: subordinate"));
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
    let (store, vfs) = store_with_vfs(tmp.path()).await;
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

// Keep a UTC timestamp reference test
#[test]
fn session_entry_created_at_is_utc() {
    let before = Utc::now();
    let entry = SessionEntry::new("test".into(), "alice".into(), 0, QuotaSnapshot::default());
    let after = Utc::now();
    assert!(entry.created_at >= before);
    assert!(entry.created_at <= after);
}
