use avix_core::executor::runtime_executor::MockToolRegistry;
use avix_core::executor::{RuntimeExecutor, SpawnParams};
use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::snapshot::{
    capture, verify_checksum, CaptureParams, CapturedBy, PendingRequest, SnapshotFile,
    SnapshotMemory, SnapshotTrigger,
};
use avix_core::types::token::CapabilityToken;
use avix_core::types::Pid;
use std::sync::Arc;

// ── helpers ──────────────────────────────────────────────────────────────────

fn minimal_params(goal: &str) -> SnapshotFile {
    capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal,
        message_history: &[],
        temperature: 0.7,
        granted_tools: &["fs/read".to_string()],
        trigger: SnapshotTrigger::Manual,
        captured_by: CapturedBy::Kernel,
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    })
}

async fn spawn_test_executor(username: &str, pid: u32) -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(pid),
        agent_name: "researcher".into(),
        goal: "Research quantum computing".into(),
        spawned_by: username.into(),
        session_id: "test-session".into(),
        token: CapabilityToken::test_token(&["fs/read", "llm/complete"]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
    };
    RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
}

// ── T-SB-01: capture() builds a valid SnapshotFile with checksum ─────────────

#[test]
fn snapshot_capture_produces_valid_file() {
    let messages = vec![
        ("user".to_string(), "Research quantum computing".to_string()),
        (
            "assistant".to_string(),
            "I'll start by searching...".to_string(),
        ),
    ];
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "Research quantum computing",
        message_history: &messages,
        temperature: 0.7,
        granted_tools: &["fs/read".to_string(), "llm/complete".to_string()],
        trigger: SnapshotTrigger::Sigsave,
        captured_by: CapturedBy::Kernel,
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    });
    assert_eq!(snap.kind, "Snapshot");
    assert_eq!(snap.metadata.agent_name, "researcher");
    assert_eq!(snap.metadata.trigger, SnapshotTrigger::Sigsave);
    assert!(snap.spec.context_token_count > 0);
    assert!(!snap.spec.checksum.is_empty());
    assert!(snap.spec.checksum.starts_with("sha256:"));
}

// ── T-SB-02: checksum changes when content changes ───────────────────────────

#[test]
fn snapshot_checksum_detects_tampering() {
    let snap1 = minimal_params("goal A");
    let snap2 = minimal_params("goal B");
    assert_ne!(snap1.spec.checksum, snap2.spec.checksum);
}

// ── T-SB-03: vfs_path() is correct ───────────────────────────────────────────

#[test]
fn snapshot_vfs_path_correct() {
    let snap = minimal_params("test");
    let path = snap.vfs_path("alice");
    assert!(
        path.starts_with("/users/alice/snapshots/researcher-"),
        "got: {path}"
    );
    assert!(path.ends_with(".yaml"), "got: {path}");
}

// ── T-SB-04: SIGSAVE writes snapshot to VFS ──────────────────────────────────

#[tokio::test]
async fn sigsave_writes_snapshot_to_vfs() {
    let vfs = Arc::new(VfsRouter::default());
    let mut executor = spawn_test_executor("alice", 57).await;
    executor = executor.with_vfs(Arc::clone(&vfs));
    executor.deliver_signal("SIGSAVE").await;

    // Snapshot file should exist somewhere under /users/alice/snapshots/
    let dir = VfsPath::parse("/users/alice/snapshots/").unwrap();
    let entries = vfs.list(&dir).await.unwrap_or_default();
    assert!(
        !entries.is_empty(),
        "expected snapshot file in VFS after SIGSAVE, got empty listing"
    );
}

// ── T-SC-01: verify_checksum passes for a freshly captured snapshot ───────────

#[test]
fn snapshot_verify_checksum_passes() {
    let snap = minimal_params("test goal");
    assert!(verify_checksum(&snap).is_ok());
}

// ── T-SC-02: verify_checksum fails for a tampered snapshot ───────────────────

#[test]
fn snapshot_verify_checksum_detects_tampering() {
    let mut snap = minimal_params("test goal");
    snap.spec.goal = "TAMPERED".into();
    assert!(verify_checksum(&snap).is_err());
    let msg = verify_checksum(&snap).unwrap_err().to_string();
    assert!(
        msg.contains("integrity"),
        "expected integrity error, got: {msg}"
    );
}

// ── T-SC-03: restore reads from VFS and rebuilds context ─────────────────────

#[tokio::test]
async fn snapshot_restore_rebuilds_context() {
    let vfs = Arc::new(VfsRouter::default());
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "Research quantum computing",
        message_history: &[],
        temperature: 0.7,
        granted_tools: &["fs/read".to_string()],
        trigger: SnapshotTrigger::Manual,
        captured_by: CapturedBy::User(1001),
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    });

    // Write to VFS
    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes())
        .await
        .unwrap();

    // Restore into a fresh executor
    let mut executor = spawn_test_executor("alice", 99).await;
    executor = executor.with_vfs(Arc::clone(&vfs));

    let result = executor.restore_from_snapshot(&snap.metadata.name).await;
    assert!(result.is_ok(), "restore failed: {result:?}");
    let r = result.unwrap();
    assert_eq!(r.agent_name, "researcher");
    assert_eq!(executor.goal(), "Research quantum computing");
}

// ── T-SC-04: restore aborts on checksum mismatch ─────────────────────────────

#[tokio::test]
async fn snapshot_restore_aborts_on_bad_checksum() {
    let vfs = Arc::new(VfsRouter::default());
    let mut snap = minimal_params("goal");
    snap.spec.goal = "TAMPERED".into(); // corrupt the content

    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes())
        .await
        .unwrap();

    let mut executor = spawn_test_executor("alice", 99).await;
    executor = executor.with_vfs(Arc::clone(&vfs));

    let result = executor.restore_from_snapshot(&snap.metadata.name).await;
    assert!(result.is_err(), "expected error on checksum mismatch");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("integrity"),
        "expected integrity error, got: {msg}"
    );
}

// ── T-SC-05: restore issues a fresh CapabilityToken ──────────────────────────

#[tokio::test]
async fn snapshot_restore_issues_fresh_token() {
    let vfs = Arc::new(VfsRouter::default());
    let granted_tools = vec!["fs/read".to_string(), "llm/complete".to_string()];
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "test",
        message_history: &[],
        temperature: 0.7,
        granted_tools: &granted_tools,
        trigger: SnapshotTrigger::Manual,
        captured_by: CapturedBy::Kernel,
        memory: SnapshotMemory::default(),
        pending_requests: vec![],
        open_pipes: vec![],
    });
    let original_tools = snap.spec.environment.granted_tools.clone();

    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes())
        .await
        .unwrap();

    let mut executor = spawn_test_executor("alice", 99).await;
    executor = executor.with_vfs(Arc::clone(&vfs));
    executor
        .restore_from_snapshot(&snap.metadata.name)
        .await
        .unwrap();

    for tool in &original_tools {
        assert!(
            executor.token().granted_tools.contains(tool),
            "fresh token missing tool '{tool}'"
        );
    }
}

// ── T-SC-06: pending requests are reported in RestoreResult ──────────────────

#[tokio::test]
async fn snapshot_restore_reports_pending_requests() {
    let vfs = Arc::new(VfsRouter::default());
    let snap = capture(CaptureParams {
        agent_name: "researcher",
        pid: 57,
        username: "alice",
        goal: "test",
        message_history: &[],
        temperature: 0.7,
        granted_tools: &["fs/read".to_string()],
        trigger: SnapshotTrigger::Manual,
        captured_by: CapturedBy::Kernel,
        memory: SnapshotMemory::default(),
        pending_requests: vec![PendingRequest {
            request_id: "req-abc".into(),
            resource: "tool".into(),
            name: "web".into(),
            status: "in-flight".into(),
        }],
        open_pipes: vec![],
    });

    let vfs_path = VfsPath::parse(&snap.vfs_path("alice")).unwrap();
    vfs.write(&vfs_path, snap.to_yaml().unwrap().into_bytes())
        .await
        .unwrap();

    let mut executor = spawn_test_executor("alice", 99).await;
    executor = executor.with_vfs(Arc::clone(&vfs));

    let result = executor
        .restore_from_snapshot(&snap.metadata.name)
        .await
        .unwrap();
    assert!(
        result.reissued_requests.contains(&"req-abc".to_string()),
        "expected req-abc in reissued_requests"
    );
}
