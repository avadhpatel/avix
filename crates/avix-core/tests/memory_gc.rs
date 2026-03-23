use avix_core::memory_svc::gc::{gc_episodic_records, prune_expired_grants};
use avix_core::memory_svc::schema::{
    MemoryGrant, MemoryGrantGrantee, MemoryGrantGrantor, MemoryGrantMetadata, MemoryGrantScope,
    MemoryGrantSpec, MemoryRecord, MemoryRecordIndex, MemoryRecordMetadata, MemoryRecordType,
    new_memory_id,
};
use avix_core::memory_svc::store;
use avix_core::memory_svc::vfs_layout::{init_user_memory_tree, memory_agent_grants_path};
use avix_core::memfs::{VfsPath, VfsRouter};
use chrono::{Duration, Utc};
use std::sync::Arc;

fn make_vfs() -> Arc<VfsRouter> {
    Arc::new(VfsRouter::new())
}

fn make_episodic_record(owner: &str, agent: &str, days_ago: i64) -> MemoryRecord {
    let created_at = Utc::now() - Duration::days(days_ago);
    let id = new_memory_id();
    MemoryRecord {
        api_version: "avix/v1".into(),
        kind: "MemoryRecord".into(),
        metadata: MemoryRecordMetadata {
            id,
            record_type: MemoryRecordType::Episodic,
            agent_name: agent.to_string(),
            agent_pid: 1,
            owner: owner.to_string(),
            created_at,
            updated_at: created_at,
            session_id: "sess-test".to_string(),
            tags: vec![],
            pinned: false,
        },
        spec: avix_core::memory_svc::schema::MemoryRecordSpec {
            content: format!("Record from {} days ago", days_ago),
            outcome: None,
            related_goal: None,
            tools_used: vec![],
            key: None,
            confidence: None,
            ttl_days: None,
            index: MemoryRecordIndex::default(),
        },
    }
}

fn make_pinned_old_record(owner: &str, agent: &str) -> MemoryRecord {
    let mut record = make_episodic_record(owner, agent, 60);
    record.metadata.pinned = true;
    record
}

async fn write_episodic(vfs: &VfsRouter, record: &MemoryRecord) -> String {
    let path = MemoryRecord::vfs_path_episodic(
        &record.metadata.owner,
        &record.metadata.agent_name,
        &record.metadata.created_at,
        &record.metadata.id,
    );
    store::write_record(vfs, &path, record).await.unwrap();
    path
}

async fn write_session_grant(vfs: &VfsRouter, agent: &str, session_id: &str, expired: bool) -> String {
    let grant_id = format!("grant-{}", new_memory_id());
    let expires_at = if expired {
        Some(Utc::now() - Duration::hours(1)) // 1 hour in the past
    } else {
        None
    };
    let grant = MemoryGrant::new(
        MemoryGrantMetadata {
            id: grant_id.clone(),
            granted_at: Utc::now(),
            granted_by: "alice".to_string(),
            hil_id: "hil-test".to_string(),
        },
        MemoryGrantSpec {
            grantor: MemoryGrantGrantor {
                agent_name: "researcher".to_string(),
                owner: "alice".to_string(),
            },
            grantee: MemoryGrantGrantee {
                agent_name: agent.to_string(),
                owner: "alice".to_string(),
            },
            records: vec!["mem-test".to_string()],
            scope: MemoryGrantScope::Session,
            session_id: session_id.to_string(),
            expires_at,
        },
    );
    let path = memory_agent_grants_path(agent, &grant_id);
    let yaml = grant.to_yaml().unwrap();
    let vfs_path = VfsPath::parse(&path).unwrap();
    vfs.write(&vfs_path, yaml.into_bytes()).await.unwrap();
    path
}

// T-MG-01: GC deletes episodic records older than retention_days
#[tokio::test]
async fn gc_deletes_old_records() {
    let vfs = make_vfs();
    init_user_memory_tree(&vfs, "alice", "researcher").await.unwrap();

    // Old record (31 days ago) — should be deleted
    let old = make_episodic_record("alice", "researcher", 31);
    let old_path = write_episodic(&vfs, &old).await;

    // Recent record (1 day ago) — should survive
    let recent = make_episodic_record("alice", "researcher", 1);
    let recent_path = write_episodic(&vfs, &recent).await;

    let report = gc_episodic_records(&vfs, &[("alice", "researcher")], 30)
        .await
        .unwrap();

    assert_eq!(report.records_deleted, 1, "expected 1 record deleted");
    assert!(
        !vfs.exists(&VfsPath::parse(&old_path).unwrap()).await,
        "old record must be deleted"
    );
    assert!(
        vfs.exists(&VfsPath::parse(&recent_path).unwrap()).await,
        "recent record must survive GC"
    );
}

// T-MG-02: GC never deletes pinned records
#[tokio::test]
async fn gc_spares_pinned_records() {
    let vfs = make_vfs();
    init_user_memory_tree(&vfs, "alice", "researcher").await.unwrap();

    let pinned = make_pinned_old_record("alice", "researcher");
    let pinned_path = write_episodic(&vfs, &pinned).await;

    let report = gc_episodic_records(&vfs, &[("alice", "researcher")], 30)
        .await
        .unwrap();

    assert_eq!(report.records_deleted, 0, "pinned records must never be GC'd");
    assert!(
        vfs.exists(&VfsPath::parse(&pinned_path).unwrap()).await,
        "pinned record must survive"
    );
}

// T-MG-01c: GC report is empty when no records exceed retention
#[tokio::test]
async fn gc_report_empty_when_no_old_records() {
    let vfs = make_vfs();
    init_user_memory_tree(&vfs, "alice", "researcher").await.unwrap();

    let fresh = make_episodic_record("alice", "researcher", 0);
    write_episodic(&vfs, &fresh).await;

    let report = gc_episodic_records(&vfs, &[("alice", "researcher")], 30)
        .await
        .unwrap();
    assert_eq!(report.records_deleted, 0);
}

// T-MG-03: prune_expired_grants removes grants with past expiresAt
#[tokio::test]
async fn prune_removes_expired_grants() {
    let vfs = make_vfs();

    // Write an expired session grant
    let expired_path = write_session_grant(&vfs, "writer", "sess-exp", true).await;

    // Write a non-expired grant (no expiresAt = never expires)
    let alive_path = write_session_grant(&vfs, "writer", "sess-alive", false).await;

    let pruned = prune_expired_grants(&vfs, &["writer"]).await.unwrap();
    assert_eq!(pruned, 1, "expected 1 expired grant pruned");

    assert!(
        !vfs.exists(&VfsPath::parse(&expired_path).unwrap()).await,
        "expired grant must be deleted"
    );
    assert!(
        vfs.exists(&VfsPath::parse(&alive_path).unwrap()).await,
        "non-expired grant must survive"
    );
}

// T-MG-03b: prune_expired_grants is a no-op when no grants dir exists
#[tokio::test]
async fn prune_no_op_when_no_grants() {
    let vfs = make_vfs();
    let pruned = prune_expired_grants(&vfs, &["nonexistent"]).await.unwrap();
    assert_eq!(pruned, 0);
}

// T-MG-gc-report: GcReport fields are correct after multi-agent run
#[tokio::test]
async fn gc_runs_across_multiple_agents() {
    let vfs = make_vfs();
    init_user_memory_tree(&vfs, "alice", "agent1").await.unwrap();
    init_user_memory_tree(&vfs, "alice", "agent2").await.unwrap();

    // Write old record for agent1
    let old1 = make_episodic_record("alice", "agent1", 35);
    write_episodic(&vfs, &old1).await;

    // Write old record for agent2
    let old2 = make_episodic_record("alice", "agent2", 40);
    write_episodic(&vfs, &old2).await;

    // Write recent record for agent1
    let fresh = make_episodic_record("alice", "agent1", 2);
    write_episodic(&vfs, &fresh).await;

    let report = gc_episodic_records(
        &vfs,
        &[("alice", "agent1"), ("alice", "agent2")],
        30,
    )
    .await
    .unwrap();
    assert_eq!(
        report.records_deleted, 2,
        "both old records across agents should be deleted"
    );
}

