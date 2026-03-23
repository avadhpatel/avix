use avix_core::config::MemoryConfig;
use avix_core::memory_svc::{
    sharing::{cleanup_session_grants, on_memory_share_approved},
    MemoryGrant, MemoryGrantScope,
};
use avix_core::memory_svc::service::{CallerContext, MemoryService};
use avix_core::memfs::{VfsPath, VfsRouter};
use serde_json::json;
use std::sync::Arc;

fn make_svc() -> (MemoryService, Arc<VfsRouter>) {
    let vfs = Arc::new(VfsRouter::new());
    let svc = MemoryService::new(Arc::clone(&vfs), Arc::new(MemoryConfig::default()));
    (svc, vfs)
}

fn make_caller_without_share(owner: &str, agent: &str) -> CallerContext {
    CallerContext {
        pid: 57,
        agent_name: agent.to_string(),
        owner: owner.to_string(),
        session_id: "sess-xyz".to_string(),
        // memory:write tools but NOT memory/share-request
        granted_tools: vec![
            "memory/retrieve".to_string(),
            "memory/log-event".to_string(),
            "memory/store-fact".to_string(),
        ],
    }
}

// T-MF-01: share-request without memory:share capability returns error
#[tokio::test]
async fn share_request_requires_memory_share_cap() {
    let (svc, _) = make_svc();
    let caller = make_caller_without_share("alice", "researcher");
    let result = svc
        .dispatch(
            "memory/share-request",
            json!({
                "targetAgent": "writer",
                "recordIds": ["mem-abc123"],
                "reason": "sharing research",
                "scope": "session"
            }),
            &caller,
        )
        .await;
    assert!(result.is_err(), "expected error when memory:share not granted");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("memory:share") || err.contains("permission") || err.contains("denied"),
        "expected permission denied error, got: {err}"
    );
}

// T-MF-03: on_memory_share_approved creates session grant in /proc/services/memory/
#[tokio::test]
async fn approved_session_grant_stored_in_proc() {
    let (svc, vfs) = make_svc();
    on_memory_share_approved(
        &svc,
        "hil-001",
        57,
        "writer",
        vec!["mem-abc123".to_string()],
        MemoryGrantScope::Session,
        "alice",
        "sess-xyz",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    let grant_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let entries = vfs.list(&grant_dir).await.unwrap();
    let yaml_entries: Vec<_> = entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        !yaml_entries.is_empty(),
        "expected grant record in /proc/services/memory/agents/writer/grants/"
    );
}

// T-MF-04: on_memory_share_approved with permanent scope stores in user memory tree
#[tokio::test]
async fn approved_permanent_grant_stored_in_user_tree() {
    let (svc, vfs) = make_svc();
    on_memory_share_approved(
        &svc,
        "hil-002",
        57,
        "writer",
        vec!["mem-def456".to_string()],
        MemoryGrantScope::Permanent,
        "alice",
        "sess-xyz",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    // Permanent grants go to /users/<owner>/memory/<grantor-agent>/grants/<id>.yaml
    let grant_dir = VfsPath::parse("/users/alice/memory/researcher/grants").unwrap();
    let entries = vfs.list(&grant_dir).await.unwrap();
    let yaml_entries: Vec<_> = entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        !yaml_entries.is_empty(),
        "expected permanent grant in /users/alice/memory/researcher/grants/"
    );
}

// T-MF-06: cleanup_session_grants removes session-scoped grants on SIGSTOP
#[tokio::test]
async fn session_cleanup_removes_session_grants() {
    let (svc, vfs) = make_svc();

    // Create a session-scoped grant
    on_memory_share_approved(
        &svc,
        "hil-001",
        57,
        "writer",
        vec!["mem-abc123".to_string()],
        MemoryGrantScope::Session,
        "alice",
        "sess-xyz",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    // Verify it's there
    let grant_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let before = vfs.list(&grant_dir).await.unwrap();
    let yaml_before: Vec<_> = before.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(!yaml_before.is_empty(), "grant should exist before cleanup");

    // Cleanup session grants for "writer" in session "sess-xyz"
    cleanup_session_grants(&svc, "writer", "sess-xyz").await.unwrap();

    let after = vfs.list(&grant_dir).await.unwrap_or_default();
    let yaml_after: Vec<_> = after.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        yaml_after.is_empty(),
        "session grants should be cleaned up after session end"
    );
}

// T-MF-06b: cleanup_session_grants leaves permanent grants intact
#[tokio::test]
async fn session_cleanup_preserves_permanent_grants() {
    let (svc, vfs) = make_svc();

    // Create a permanent grant for "writer"
    on_memory_share_approved(
        &svc,
        "hil-perm",
        57,
        "writer",
        vec!["mem-perm123".to_string()],
        MemoryGrantScope::Permanent,
        "alice",
        "sess-xyz",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    // Also create a session grant for "writer"
    on_memory_share_approved(
        &svc,
        "hil-sess",
        57,
        "writer",
        vec!["mem-sess456".to_string()],
        MemoryGrantScope::Session,
        "alice",
        "sess-xyz",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    // Cleanup session grants — permanent should survive
    cleanup_session_grants(&svc, "writer", "sess-xyz").await.unwrap();

    let session_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let session_entries = vfs.list(&session_dir).await.unwrap_or_default();
    let session_yaml: Vec<_> = session_entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        session_yaml.is_empty(),
        "session grants dir should be empty after cleanup"
    );

    // Permanent grant lives in user tree — should still be there
    let perm_dir = VfsPath::parse("/users/alice/memory/researcher/grants").unwrap();
    let perm_entries = vfs.list(&perm_dir).await.unwrap();
    let perm_yaml: Vec<_> = perm_entries.iter().filter(|e| e.ends_with(".yaml")).collect();
    assert!(
        !perm_yaml.is_empty(),
        "permanent grants should survive session cleanup"
    );
}

// T-MF-grant-fields: MemoryGrant record has correct fields
#[tokio::test]
async fn grant_record_has_correct_fields() {
    let (svc, vfs) = make_svc();
    on_memory_share_approved(
        &svc,
        "hil-fields",
        99,
        "writer",
        vec!["mem-r1".to_string(), "mem-r2".to_string()],
        MemoryGrantScope::Session,
        "alice",
        "sess-abc",
        "alice",
        "researcher",
    )
    .await
    .unwrap();

    let grant_dir = VfsPath::parse("/proc/services/memory/agents/writer/grants").unwrap();
    let entries = vfs.list(&grant_dir).await.unwrap();
    let grant_file = entries.iter().find(|e| e.ends_with(".yaml")).unwrap();
    let full_path = format!(
        "/proc/services/memory/agents/writer/grants/{}",
        grant_file
    );
    let bytes = vfs
        .read(&VfsPath::parse(&full_path).unwrap())
        .await
        .unwrap();
    let yaml = String::from_utf8(bytes).unwrap();
    let grant = MemoryGrant::from_yaml(&yaml).unwrap();

    assert_eq!(grant.kind, "MemoryGrant");
    assert_eq!(grant.spec.grantor.agent_name, "researcher");
    assert_eq!(grant.spec.grantor.owner, "alice");
    assert_eq!(grant.spec.grantee.agent_name, "writer");
    assert_eq!(grant.spec.scope, MemoryGrantScope::Session);
    assert_eq!(grant.spec.session_id, "sess-abc");
    assert_eq!(grant.metadata.hil_id, "hil-fields");
    assert_eq!(grant.metadata.granted_by, "alice");
    assert_eq!(grant.spec.records, vec!["mem-r1", "mem-r2"]);
}
