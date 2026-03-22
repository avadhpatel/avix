use avix_core::session::{SessionEntry, SessionStore};
use chrono::Utc;
use serde_json::json;
use tempfile::tempdir;

fn make_entry(id: &str) -> SessionEntry {
    SessionEntry {
        session_id: id.to_string(),
        agent_name: "researcher".to_string(),
        goal: "find the answer".to_string(),
        messages: vec![json!({"role": "user", "content": "hello"})],
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
