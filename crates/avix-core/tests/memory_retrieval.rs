use avix_core::config::MemoryConfig;
use avix_core::memfs::VfsRouter;
use avix_core::memory_svc::{
    index::{is_vector_index_stale, rrf_merge, VectorEntry, VectorIndex},
    service::{CallerContext, MemoryService},
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;

fn make_svc() -> (MemoryService, Arc<VfsRouter>) {
    let vfs = Arc::new(VfsRouter::new());
    let svc = MemoryService::new(Arc::clone(&vfs), Arc::new(MemoryConfig::default()));
    (svc, vfs)
}

fn make_caller(owner: &str, agent: &str) -> CallerContext {
    CallerContext {
        pid: 1,
        agent_name: agent.to_string(),
        owner: owner.to_string(),
        session_id: "sess-test".to_string(),
        granted_tools: vec![
            "memory/retrieve".to_string(),
            "memory/log-event".to_string(),
            "memory/store-fact".to_string(),
            "memory/get-fact".to_string(),
            "memory/get-preferences".to_string(),
            "memory/forget".to_string(),
        ],
    }
}

// T-ME-01: rrf_merge deduplicates and ranks by score
#[test]
fn rrf_merge_deduplicates() {
    let bm25 = vec![("a".to_string(), 0.9f32), ("b".to_string(), 0.7)];
    let vector = vec![("b".to_string(), 0.95f32), ("c".to_string(), 0.8)];
    let merged = rrf_merge(bm25, vector, 60);
    // b appears in both lists → should rank first due to RRF boost
    assert!(
        merged.contains(&"b".to_string()),
        "b should be in merged (appears in both lists)"
    );
    assert_eq!(
        merged[0], "b",
        "b should rank first (appears in both lists)"
    );
    // a and c should appear too
    assert!(merged.contains(&"a".to_string()));
    assert!(merged.contains(&"c".to_string()));
}

// T-ME-01b: rrf_merge handles empty inputs
#[test]
fn rrf_merge_handles_empty() {
    let merged = rrf_merge(vec![], vec![], 60);
    assert!(merged.is_empty());

    let merged = rrf_merge(vec![("x".to_string(), 1.0)], vec![], 60);
    assert_eq!(merged, vec!["x".to_string()]);
}

// T-ME-02: stale vector index detected correctly
#[test]
fn stale_vector_index_detected() {
    let idx = VectorIndex {
        model: "old-model".into(),
        entries: vec![],
    };
    assert!(is_vector_index_stale(&idx, "new-model"));
    assert!(!is_vector_index_stale(&idx, "old-model"));
}

// T-ME-02b: empty model name is considered stale
#[test]
fn empty_model_is_stale() {
    let idx = VectorIndex {
        model: String::new(),
        entries: vec![],
    };
    assert!(is_vector_index_stale(&idx, "any-model"));
}

// T-ME-03: retrieve falls back to BM25 when no vector index exists
#[tokio::test]
async fn retrieve_falls_back_to_bm25_when_no_vector_index() {
    let (svc, _) = make_svc();
    let caller = make_caller("alice", "researcher");
    // Store some records
    svc.dispatch(
        "memory/log-event",
        json!({ "summary": "Quantum computing breakthrough.", "scope": "own" }),
        &caller,
    )
    .await
    .unwrap();
    // Retrieve without vector index — should still work via BM25
    let result = svc
        .dispatch(
            "memory/retrieve",
            json!({ "query": "quantum computing", "limit": 5 }),
            &caller,
        )
        .await
        .unwrap();
    assert!(result["returned"].as_u64().unwrap() > 0);
}

// T-ME-05: retrieve omits relevance field when no LLM re-rank
#[tokio::test]
async fn retrieve_omits_relevance_without_llm() {
    let (svc, _) = make_svc();
    let caller = make_caller("alice", "researcher");
    svc.dispatch(
        "memory/log-event",
        json!({ "summary": "Quantum computing breakthrough.", "scope": "own" }),
        &caller,
    )
    .await
    .unwrap();
    let result = svc
        .dispatch(
            "memory/retrieve",
            json!({ "query": "quantum", "limit": 5 }),
            &caller,
        )
        .await
        .unwrap();
    let records = result["records"].as_array().unwrap();
    if !records.is_empty() {
        // Without LLM re-rank, relevance field should not be present
        assert!(
            !records[0].as_object().unwrap().contains_key("relevance"),
            "expected no relevance field without LLM re-rank"
        );
    }
}

// T-ME-06: VectorIndex round-trips through JSON
#[test]
fn vector_index_round_trips() {
    let idx = VectorIndex {
        model: "text-embedding-3-small".into(),
        entries: vec![VectorEntry {
            id: "mem-abc".into(),
            vector: vec![0.1, 0.2, 0.3],
            updated_at: Utc::now(),
        }],
    };
    let json = serde_json::to_string(&idx).unwrap();
    let parsed: VectorIndex = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.model, "text-embedding-3-small");
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].id, "mem-abc");
    assert!((parsed.entries[0].vector[0] - 0.1f32).abs() < 1e-6);
}
