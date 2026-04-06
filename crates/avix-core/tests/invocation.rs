use avix_core::invocation::{InvocationRecord, InvocationStatus, InvocationStore};
use avix_core::memfs::local_provider::LocalProvider;
use chrono::Utc;
use tempfile::tempdir;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn open_store(dir: &std::path::Path) -> InvocationStore {
    InvocationStore::open(dir.join("invocations.redb"))
        .await
        .unwrap()
}

async fn open_store_with_local(dir: &std::path::Path) -> InvocationStore {
    let provider = LocalProvider::new(dir).unwrap();
    InvocationStore::open(dir.join("invocations.redb"))
        .await
        .unwrap()
        .with_local(provider)
}

fn make_record(id: &str, username: &str, agent: &str, session_id: &str) -> InvocationRecord {
    InvocationRecord::new(
        id.into(),
        agent.into(),
        username.into(),
        42,
        "accomplish the goal".into(),
        session_id.into(),
    )
}

// ── Basic CRUD ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_get_roundtrip() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    let rec = make_record("inv-001", "alice", "researcher", "sess-1");
    store.create(&rec).await.unwrap();

    let loaded = store.get("inv-001").await.unwrap().unwrap();
    assert_eq!(loaded.id, "inv-001");
    assert_eq!(loaded.username, "alice");
    assert_eq!(loaded.agent_name, "researcher");
    assert_eq!(loaded.session_id, "sess-1");
    assert_eq!(loaded.status, InvocationStatus::Running);
    assert!(loaded.ended_at.is_none());
}

#[tokio::test]
async fn get_missing_returns_none() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    assert!(store.get("no-such-id").await.unwrap().is_none());
}

#[tokio::test]
async fn persists_across_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("invocations.redb");
    {
        let store = InvocationStore::open(&db_path).await.unwrap();
        store
            .create(&make_record("inv-reopen", "alice", "researcher", "s1"))
            .await
            .unwrap();
    }
    // Reopen and verify record is still there.
    let store2 = InvocationStore::open(&db_path).await.unwrap();
    let loaded = store2.get("inv-reopen").await.unwrap().unwrap();
    assert_eq!(loaded.id, "inv-reopen");
    assert_eq!(loaded.status, InvocationStatus::Running);
}

// ── Status transitions ────────────────────────────────────────────────────────

#[tokio::test]
async fn finalize_completed_sets_terminal_fields() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    store
        .create(&make_record("inv-fin", "alice", "coder", "s1"))
        .await
        .unwrap();

    let ended = Utc::now();
    store
        .finalize(
            "inv-fin",
            InvocationStatus::Completed,
            ended,
            12_000,
            25,
            None,
        )
        .await
        .unwrap();

    let loaded = store.get("inv-fin").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Completed);
    assert!(loaded.ended_at.is_some());
    assert_eq!(loaded.tokens_consumed, 12_000);
    assert_eq!(loaded.tool_calls_total, 25);
    assert!(loaded.exit_reason.is_none());
}

#[tokio::test]
async fn finalize_failed_records_exit_reason() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    store
        .create(&make_record("inv-fail", "alice", "coder", "s1"))
        .await
        .unwrap();

    store
        .finalize(
            "inv-fail",
            InvocationStatus::Failed,
            Utc::now(),
            500,
            3,
            Some("token limit exceeded".into()),
        )
        .await
        .unwrap();

    let loaded = store.get("inv-fail").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Failed);
    assert_eq!(
        loaded.exit_reason.as_deref(),
        Some("token limit exceeded")
    );
}

#[tokio::test]
async fn finalize_killed_records_status() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    store
        .create(&make_record("inv-kill", "alice", "researcher", "s1"))
        .await
        .unwrap();

    store
        .finalize(
            "inv-kill",
            InvocationStatus::Killed,
            Utc::now(),
            0,
            0,
            Some("killed".into()),
        )
        .await
        .unwrap();

    let loaded = store.get("inv-kill").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Killed);
}

#[tokio::test]
async fn update_status_to_idle() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    store
        .create(&make_record("inv-idle", "alice", "researcher", "s1"))
        .await
        .unwrap();

    store
        .update_status("inv-idle", InvocationStatus::Idle)
        .await
        .unwrap();

    let loaded = store.get("inv-idle").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Idle);
    // Idle is non-terminal — ended_at must not be set.
    assert!(loaded.ended_at.is_none());
}

#[tokio::test]
async fn update_status_to_paused_and_back_to_running() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    store
        .create(&make_record("inv-pause", "alice", "researcher", "s1"))
        .await
        .unwrap();

    store
        .update_status("inv-pause", InvocationStatus::Paused)
        .await
        .unwrap();
    let loaded = store.get("inv-pause").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Paused);
    assert!(loaded.ended_at.is_none());

    store
        .update_status("inv-pause", InvocationStatus::Running)
        .await
        .unwrap();
    let loaded = store.get("inv-pause").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Running);
}

#[tokio::test]
async fn finalize_unknown_id_is_idempotent() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    let result = store
        .finalize(
            "no-such",
            InvocationStatus::Completed,
            Utc::now(),
            0,
            0,
            None,
        )
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn update_status_unknown_id_is_idempotent() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    let result = store
        .update_status("no-such", InvocationStatus::Idle)
        .await;
    assert!(result.is_ok());
}

// ── List / filter ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_for_user_filters_correctly() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;

    store
        .create(&make_record("a1", "alice", "researcher", "s1"))
        .await
        .unwrap();
    store
        .create(&make_record("a2", "alice", "coder", "s1"))
        .await
        .unwrap();
    store
        .create(&make_record("b1", "bob", "researcher", "s2"))
        .await
        .unwrap();

    let alice = store.list_for_user("alice").await.unwrap();
    assert_eq!(alice.len(), 2);
    assert!(alice.iter().all(|r| r.username == "alice"));

    let bob = store.list_for_user("bob").await.unwrap();
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].id, "b1");
}

#[tokio::test]
async fn list_for_agent_filters_correctly() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;

    store
        .create(&make_record("r1", "alice", "researcher", "s1"))
        .await
        .unwrap();
    store
        .create(&make_record("r2", "alice", "researcher", "s2"))
        .await
        .unwrap();
    store
        .create(&make_record("c1", "alice", "coder", "s1"))
        .await
        .unwrap();

    let result = store.list_for_agent("alice", "researcher").await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|r| r.agent_name == "researcher"));

    let coders = store.list_for_agent("alice", "coder").await.unwrap();
    assert_eq!(coders.len(), 1);
}

#[tokio::test]
async fn list_all_spans_users() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;

    store
        .create(&make_record("x1", "alice", "bot", "s1"))
        .await
        .unwrap();
    store
        .create(&make_record("x2", "bob", "bot", "s2"))
        .await
        .unwrap();
    store
        .create(&make_record("x3", "carol", "bot", "s3"))
        .await
        .unwrap();

    let all = store.list_all().await.unwrap();
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn multiple_invocations_same_session_no_collision() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;

    store
        .create(&make_record("inv-sa", "alice", "researcher", "shared-sess"))
        .await
        .unwrap();
    store
        .create(&make_record("inv-sb", "alice", "coder", "shared-sess"))
        .await
        .unwrap();

    let all = store.list_for_user("alice").await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|r| r.session_id == "shared-sess"));
}

// ── Disk artefacts ────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_writes_yaml_artefact() {
    let dir = tempdir().unwrap();
    let store = open_store_with_local(dir.path()).await;
    let rec = make_record("inv-yaml", "alice", "researcher", "s1");
    store.create(&rec).await.unwrap();

    let path = dir
        .path()
        .join("alice/agents/researcher/invocations/inv-yaml.yaml");
    assert!(path.exists(), "YAML artefact must be written on create");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("inv-yaml"));
    assert!(content.contains("alice"));
    assert!(content.contains("researcher"));
    assert!(content.contains("running"));
}

#[tokio::test]
async fn finalize_updates_yaml_artefact() {
    let dir = tempdir().unwrap();
    let store = open_store_with_local(dir.path()).await;
    let rec = make_record("inv-fyaml", "alice", "coder", "s1");
    store.create(&rec).await.unwrap();

    store
        .finalize(
            "inv-fyaml",
            InvocationStatus::Completed,
            Utc::now(),
            8_000,
            10,
            None,
        )
        .await
        .unwrap();

    let path = dir
        .path()
        .join("alice/agents/coder/invocations/inv-fyaml.yaml");
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("completed"), "YAML must show completed status");
}

#[tokio::test]
async fn write_conversation_creates_jsonl_at_correct_path() {
    let dir = tempdir().unwrap();
    let store = open_store_with_local(dir.path()).await;
    let rec = make_record("inv-conv", "alice", "researcher", "s1");
    store.create(&rec).await.unwrap();

    let messages = vec![
        ("user".into(), "What is the capital of France?".into()),
        ("assistant".into(), "Paris.".into()),
        ("user".into(), "Tell me more.".into()),
        ("assistant".into(), "Paris is the capital city...".into()),
    ];
    store
        .write_conversation("inv-conv", "alice", "researcher", &messages)
        .await
        .unwrap();

    let path = dir
        .path()
        .join("alice/agents/researcher/invocations/inv-conv/conversation.jsonl");
    assert!(path.exists(), "conversation.jsonl must exist");

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 4);

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["role"], "user");
    assert_eq!(first["content"], "What is the capital of France?");

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["role"], "assistant");
    assert_eq!(second["content"], "Paris.");
}

#[tokio::test]
async fn persist_interim_updates_tokens_without_finalizing() {
    let dir = tempdir().unwrap();
    let store = open_store_with_local(dir.path()).await;
    let rec = make_record("inv-interim", "alice", "researcher", "s1");
    store.create(&rec).await.unwrap();

    let messages = vec![("user".into(), "Hello".into())];
    store
        .persist_interim("inv-interim", &messages, 3_000, 7)
        .await
        .unwrap();

    let loaded = store.get("inv-interim").await.unwrap().unwrap();
    assert_eq!(loaded.tokens_consumed, 3_000);
    assert_eq!(loaded.tool_calls_total, 7);
    // Non-terminal — status and ended_at must be unchanged.
    assert_eq!(loaded.status, InvocationStatus::Running);
    assert!(loaded.ended_at.is_none());
}

// ── Full lifecycle ────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_running_to_idle_to_completed() {
    let dir = tempdir().unwrap();
    let store = open_store_with_local(dir.path()).await;
    let rec = make_record("inv-lc", "alice", "researcher", "sess-lc");
    store.create(&rec).await.unwrap();

    // Turn 1 finishes — agent waits for input (Idle).
    store
        .update_status("inv-lc", InvocationStatus::Idle)
        .await
        .unwrap();
    let loaded = store.get("inv-lc").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Idle);
    assert!(loaded.ended_at.is_none());

    // User sends a follow-up — agent resumes (Running).
    store
        .update_status("inv-lc", InvocationStatus::Running)
        .await
        .unwrap();
    let loaded = store.get("inv-lc").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Running);

    // Agent finishes the session (Completed).
    store
        .finalize(
            "inv-lc",
            InvocationStatus::Completed,
            Utc::now(),
            20_000,
            42,
            None,
        )
        .await
        .unwrap();
    let loaded = store.get("inv-lc").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Completed);
    assert!(loaded.ended_at.is_some());
    assert_eq!(loaded.tokens_consumed, 20_000);
}

#[tokio::test]
async fn full_lifecycle_with_pause_and_resume() {
    let dir = tempdir().unwrap();
    let store = open_store(dir.path()).await;
    let rec = make_record("inv-pr", "alice", "researcher", "sess-pr");
    store.create(&rec).await.unwrap();

    store
        .update_status("inv-pr", InvocationStatus::Paused)
        .await
        .unwrap();
    let loaded = store.get("inv-pr").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Paused);

    store
        .update_status("inv-pr", InvocationStatus::Running)
        .await
        .unwrap();
    let loaded = store.get("inv-pr").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Running);

    store
        .finalize(
            "inv-pr",
            InvocationStatus::Completed,
            Utc::now(),
            5_000,
            8,
            None,
        )
        .await
        .unwrap();
    let loaded = store.get("inv-pr").await.unwrap().unwrap();
    assert_eq!(loaded.status, InvocationStatus::Completed);
}
