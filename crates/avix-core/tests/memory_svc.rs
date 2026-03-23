use avix_core::memory_svc::{
    service::{CallerContext, MemoryService},
    search::bm25_rank,
    schema::{MemoryRecord, MemoryRecordIndex, MemoryRecordMetadata, MemoryRecordSpec, MemoryRecordType},
};
use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::config::MemoryConfig;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

fn make_test_memory_svc() -> (MemoryService, Arc<VfsRouter>) {
    let vfs = Arc::new(VfsRouter::new());
    let config = Arc::new(MemoryConfig::default());
    let svc = MemoryService::new(Arc::clone(&vfs), config);
    (svc, vfs)
}

fn make_caller(owner: &str, agent_name: &str, pid: u32) -> CallerContext {
    CallerContext {
        pid,
        agent_name: agent_name.to_string(),
        owner: owner.to_string(),
        session_id: "sess-test".to_string(),
        granted_tools: vec![
            "memory/log-event".to_string(),
            "memory/store-fact".to_string(),
            "memory/get-fact".to_string(),
            "memory/update-preference".to_string(),
            "memory/get-preferences".to_string(),
            "memory/forget".to_string(),
            "memory/retrieve".to_string(),
        ],
    }
}

fn make_test_record(content: &str) -> MemoryRecord {
    MemoryRecord::new(
        MemoryRecordMetadata {
            id: format!("mem-test{}", content.len()),
            record_type: MemoryRecordType::Episodic,
            agent_name: "researcher".into(),
            agent_pid: 1,
            owner: "alice".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            session_id: "sess-1".into(),
            tags: vec![],
            pinned: false,
        },
        MemoryRecordSpec {
            content: content.into(),
            outcome: None,
            related_goal: None,
            tools_used: vec![],
            key: None,
            confidence: None,
            ttl_days: None,
            index: MemoryRecordIndex::default(),
        },
    )
}

// T-MC-01: memory/log-event stores a record to VFS
#[tokio::test]
async fn log_event_stores_to_vfs() {
    let (svc, vfs) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc
        .dispatch(
            "memory/log-event",
            json!({
                "summary": "Completed research on quantum computing.",
                "outcome": "success",
                "tags": ["research", "quantum"],
                "pinned": false,
                "scope": "own"
            }),
            &caller,
        )
        .await
        .unwrap();
    assert_eq!(result["stored"], true);
    let id = result["id"].as_str().unwrap();
    assert!(id.starts_with("mem-"));
    // Verify file exists in VFS
    let episodic_dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    let entries = vfs.list(&episodic_dir).await.unwrap();
    assert!(entries.iter().any(|e| e.contains(id)));
}

// T-MC-02: memory/store-fact writes semantic record and replaces on second write
#[tokio::test]
async fn store_fact_writes_and_replaces_semantic_record() {
    let (svc, _vfs) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc
        .dispatch(
            "memory/store-fact",
            json!({
                "key": "project-alpha-deadline",
                "summary": "Project Alpha deadline is April 30, 2026.",
                "confidence": "high",
                "pinned": true,
                "scope": "own"
            }),
            &caller,
        )
        .await
        .unwrap();
    assert_eq!(result["stored"], true);
    assert_eq!(result["replaced"], false);
    // Second write should be a replace
    let result2 = svc
        .dispatch(
            "memory/store-fact",
            json!({
                "key": "project-alpha-deadline",
                "summary": "Updated: deadline moved to May 1.",
                "confidence": "high",
                "scope": "own"
            }),
            &caller,
        )
        .await
        .unwrap();
    assert_eq!(result2["stored"], true);
    assert_eq!(result2["replaced"], true);
}

// T-MC-03: memory/get-fact returns stored record by key
#[tokio::test]
async fn get_fact_returns_by_key() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    // Store it first
    svc.dispatch(
        "memory/store-fact",
        json!({
            "key": "pi-value",
            "summary": "Pi is approximately 3.14159.",
            "confidence": "high",
            "scope": "own"
        }),
        &caller,
    )
    .await
    .unwrap();
    // Retrieve by key
    let result = svc
        .dispatch("memory/get-fact", json!({ "key": "pi-value" }), &caller)
        .await
        .unwrap();
    assert_eq!(result["found"], true);
    assert!(result["record"]["summary"]
        .as_str()
        .unwrap()
        .contains("3.14159"));
}

// T-MC-03b: get-fact returns not-found for missing key
#[tokio::test]
async fn get_fact_not_found_for_missing_key() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc
        .dispatch("memory/get-fact", json!({ "key": "nonexistent" }), &caller)
        .await
        .unwrap();
    assert_eq!(result["found"], false);
}

// T-MC-04: memory/update-preference merges into existing model
#[tokio::test]
async fn update_preference_merges() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    // First update
    let r1 = svc
        .dispatch(
            "memory/update-preference",
            json!({ "summary": "Prefers concise answers." }),
            &caller,
        )
        .await
        .unwrap();
    assert_eq!(r1["updated"], true);
    // Second update — merges summary
    let r2 = svc
        .dispatch(
            "memory/update-preference",
            json!({ "summary": "Prefers concise answers with examples." }),
            &caller,
        )
        .await
        .unwrap();
    assert_eq!(r2["updated"], true);
    // Verify get-preferences returns the latest summary
    let prefs = svc
        .dispatch("memory/get-preferences", json!({}), &caller)
        .await
        .unwrap();
    assert_eq!(prefs["found"], true);
    assert!(prefs["model"]["spec"]["summary"]
        .as_str()
        .unwrap()
        .contains("examples"));
}

// T-MC-05: memory/get-preferences returns not-found for new agent
#[tokio::test]
async fn get_preferences_not_found_for_new_agent() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "brand-new-agent", 1);
    let result = svc
        .dispatch("memory/get-preferences", json!({}), &caller)
        .await
        .unwrap();
    assert_eq!(result["found"], false);
}

// T-MC-06: memory/forget deletes by ID
#[tokio::test]
async fn forget_deletes_by_id() {
    let (svc, vfs) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    // Store a record
    let stored = svc
        .dispatch(
            "memory/log-event",
            json!({ "summary": "Something to forget.", "scope": "own" }),
            &caller,
        )
        .await
        .unwrap();
    let id = stored["id"].as_str().unwrap().to_string();
    // Verify it exists
    let episodic_dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    let before = vfs.list(&episodic_dir).await.unwrap();
    assert!(before.iter().any(|e| e.contains(&id)));
    // Forget it
    let result = svc
        .dispatch("memory/forget", json!({ "ids": [id.clone()] }), &caller)
        .await
        .unwrap();
    assert!(result["deleted"].as_array().unwrap().iter().any(|v| v.as_str() == Some(&id)));
    assert!(result["notFound"].as_array().unwrap().is_empty());
}

// T-MC-07: memory/retrieve returns BM25-ranked results
#[tokio::test]
async fn retrieve_returns_ranked_results() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    svc.dispatch(
        "memory/log-event",
        json!({
            "summary": "Quantum computing research completed. Topological qubits discovered.",
            "scope": "own"
        }),
        &caller,
    )
    .await
    .unwrap();
    svc.dispatch(
        "memory/log-event",
        json!({
            "summary": "Financial analysis. Q3 OPEX anomalies found.",
            "scope": "own"
        }),
        &caller,
    )
    .await
    .unwrap();
    let result = svc
        .dispatch(
            "memory/retrieve",
            json!({ "query": "quantum computing", "limit": 5 }),
            &caller,
        )
        .await
        .unwrap();
    let records = result["records"].as_array().unwrap();
    assert!(!records.is_empty());
    assert!(records[0]["summary"]
        .as_str()
        .unwrap()
        .contains("Quantum"));
}

// T-MC-08: unknown tool returns NotFound error
#[tokio::test]
async fn unknown_tool_returns_error() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc
        .dispatch("memory/does-not-exist", json!({}), &caller)
        .await;
    assert!(result.is_err());
}

// T-MC-09: BM25 rank returns empty for no-match query
#[test]
fn bm25_rank_empty_for_no_match() {
    let records = vec![make_test_record("quantum computing research")];
    let ranked = bm25_rank(&records, "financial analysis quarterly budget", 5);
    // Should be empty — no token overlap
    assert!(ranked.is_empty());
}
