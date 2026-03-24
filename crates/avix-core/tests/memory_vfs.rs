/// Integration tests for memory VFS layout (memory-gap-B).
use avix_core::bootstrap::phase1;
use avix_core::memfs::{VfsPath, VfsRouter};
use avix_core::memory_svc::vfs_layout::init_user_memory_tree;

// T-MB-01: memory tree paths are NOT agent-writable
#[test]
fn memory_tree_not_agent_writable() {
    let paths = [
        "/users/alice/memory/researcher/episodic/2026-03-22T14:30:00Z-abc.yaml",
        "/users/alice/memory/researcher/semantic/project-alpha.yaml",
        "/users/alice/memory/researcher/preferences/user-model.yaml",
        "/crews/analysts/memory/shared/episodic/some.yaml",
    ];
    for p in &paths {
        let vfs_path = VfsPath::parse(p).unwrap();
        assert!(
            !vfs_path.is_agent_writable(),
            "expected non-writable by agent: {p}"
        );
    }
}

// T-MB-02: workspace paths remain agent-writable
#[test]
fn workspace_paths_still_writable() {
    let path = VfsPath::parse("/users/alice/workspace/report.md").unwrap();
    assert!(path.is_agent_writable());
}

// T-MB-03: ensure_dir is idempotent
#[tokio::test]
async fn ensure_dir_idempotent() {
    let vfs = VfsRouter::new();
    let dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    vfs.ensure_dir(&dir).await.unwrap();
    vfs.ensure_dir(&dir).await.unwrap(); // second call must not error
    let entries = vfs.list(&dir).await.unwrap();
    assert!(!entries.is_empty(), "expected .keep anchor");
}

// T-MB-04: init_user_memory_tree creates all required subdirs
#[tokio::test]
async fn init_user_memory_tree_creates_dirs() {
    let vfs = VfsRouter::new();
    init_user_memory_tree(&vfs, "alice", "researcher")
        .await
        .unwrap();
    for dir in &[
        "/users/alice/memory/researcher/episodic",
        "/users/alice/memory/researcher/semantic",
        "/users/alice/memory/researcher/preferences",
        "/users/alice/memory/researcher/grants",
        "/users/alice/memory/researcher/episodic/index",
        "/users/alice/memory/researcher/semantic/index",
    ] {
        assert!(
            vfs.exists(&VfsPath::parse(&format!("{}/.keep", dir)).unwrap())
                .await,
            "expected dir anchor at {dir}"
        );
    }
}

// T-MB-05: phase1 bootstrap creates /proc/services/memory/
#[tokio::test]
async fn phase1_creates_memory_svc_proc_dirs() {
    let vfs = VfsRouter::new();
    phase1::run(&vfs).await;
    assert!(
        vfs.exists(&VfsPath::parse("/proc/services/memory/agents/.keep").unwrap())
            .await,
        "expected /proc/services/memory/agents/ to be created at bootstrap"
    );
}

// T-MB-06: MemorySvcStatus round-trips through YAML
#[test]
fn memory_svc_status_round_trips() {
    use avix_core::memory_svc::vfs_layout::MemorySvcStatus;
    use chrono::Utc;
    let status = MemorySvcStatus {
        healthy: true,
        total_episodic_records: 1234,
        total_semantic_records: 567,
        active_session_grants: 2,
        updated_at: Utc::now(),
    };
    let yaml = serde_yaml::to_string(&status).unwrap();
    let parsed: MemorySvcStatus = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(parsed.total_episodic_records, 1234);
    assert!(parsed.healthy);
}
