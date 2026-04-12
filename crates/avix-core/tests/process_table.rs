use avix_core::process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable};
use avix_core::types::Pid;
use chrono::Utc;
use std::sync::Arc;

fn make_agent_entry(pid: u64, name: &str) -> ProcessEntry {
    ProcessEntry {
        pid: Pid::from_u64(pid),
        name: name.to_string(),
        kind: ProcessKind::Agent,
        status: ProcessStatus::Running,
        parent: None,
        spawned_by_user: "alice".to_string(),
        ..Default::default()
    }
}

fn make_service_entry(pid: u64, name: &str) -> ProcessEntry {
    ProcessEntry {
        pid: Pid::from_u64(pid),
        name: name.to_string(),
        kind: ProcessKind::Service,
        status: ProcessStatus::Running,
        parent: None,
        spawned_by_user: "system".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn insert_and_lookup_by_pid() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    let entry = table.get(Pid::from_u64(57)).await.unwrap();
    assert_eq!(entry.name, "researcher");
}

#[tokio::test]
async fn lookup_missing_pid_returns_none() {
    let table = ProcessTable::new();
    assert!(table.get(Pid::from_u64(99)).await.is_none());
}

#[tokio::test]
async fn remove_entry() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.remove(Pid::from_u64(57)).await;
    assert!(table.get(Pid::from_u64(57)).await.is_none());
}

#[tokio::test]
async fn remove_nonexistent_is_noop() {
    let table = ProcessTable::new();
    table.remove(Pid::from_u64(999)).await;
}

#[tokio::test]
async fn update_status() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table
        .set_status(Pid::from_u64(57), ProcessStatus::Paused)
        .await
        .unwrap();
    let entry = table.get(Pid::from_u64(57)).await.unwrap();
    assert_eq!(entry.status, ProcessStatus::Paused);
}

#[tokio::test]
async fn update_status_missing_pid_returns_err() {
    let table = ProcessTable::new();
    let result = table.set_status(Pid::from_u64(99), ProcessStatus::Paused).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn list_all() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.insert(make_agent_entry(58, "writer")).await;
    table.insert(make_service_entry(2, "router")).await;
    let all = table.list_all().await;
    assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn list_agents_only() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    table.insert(make_service_entry(2, "router")).await;
    let agents = table.list_by_kind(ProcessKind::Agent).await;
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "researcher");
}

#[tokio::test]
async fn list_by_parent() {
    let table = ProcessTable::new();
    let mut child = make_agent_entry(58, "child");
    child.parent = Some(Pid::from_u64(57));
    table.insert(make_agent_entry(57, "parent")).await;
    table.insert(child).await;
    let children = table.list_children(Pid::from_u64(57)).await;
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].pid, Pid::from_u64(58));
}

#[tokio::test]
async fn list_by_status() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "running-agent")).await;
    let mut paused = make_agent_entry(58, "paused-agent");
    paused.status = ProcessStatus::Paused;
    table.insert(paused).await;
    let running = table.list_by_status(ProcessStatus::Running).await;
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].name, "running-agent");
}

#[tokio::test]
async fn find_by_name() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(57, "researcher")).await;
    let found = table.find_by_name("researcher").await.unwrap();
    assert_eq!(found.pid, Pid::from_u64(57));
}

#[tokio::test]
async fn find_by_name_missing_returns_none() {
    let table = ProcessTable::new();
    assert!(table.find_by_name("ghost").await.is_none());
}

#[tokio::test]
async fn concurrent_inserts_all_visible() {
    let table = Arc::new(ProcessTable::new());
    let mut handles = Vec::new();
    for i in 0..100u64 {
        let t = Arc::clone(&table);
        handles.push(tokio::spawn(async move {
            t.insert(make_agent_entry(i + 100, &format!("agent-{i}")))
                .await;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(table.list_all().await.len(), 100);
}

#[tokio::test]
async fn concurrent_reads_do_not_block_each_other() {
    let table = Arc::new(ProcessTable::new());
    table.insert(make_agent_entry(57, "researcher")).await;
    let mut handles = Vec::new();
    for _ in 0..50 {
        let t = Arc::clone(&table);
        handles.push(tokio::spawn(
            async move { t.get(Pid::from_u64(57)).await.is_some() },
        ));
    }
    let results: Vec<_> = futures::future::join_all(handles).await;
    assert!(results.iter().all(|r| *r.as_ref().unwrap()));
}

#[tokio::test]
async fn count_is_accurate() {
    let table = ProcessTable::new();
    assert_eq!(table.count().await, 0);
    table.insert(make_agent_entry(57, "a")).await;
    assert_eq!(table.count().await, 1);
    table.remove(Pid::from_u64(57)).await;
    assert_eq!(table.count().await, 0);
}

// ── Token / chain-depth fields ──────────────────────────────────────────

#[tokio::test]
async fn set_token_stores_granted_tools_and_expiry() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(70, "researcher")).await;
    let expires = Utc::now() + chrono::Duration::hours(1);
    table
        .set_token(
            Pid::from_u64(70),
            vec!["fs/read".into(), "agent/spawn".into()],
            Some(expires),
        )
        .await
        .unwrap();

    let entry = table.get(Pid::from_u64(70)).await.unwrap();
    assert_eq!(entry.granted_tools, vec!["fs/read", "agent/spawn"]);
    assert!(entry.token_expires_at.is_some());
}

#[tokio::test]
async fn set_token_missing_pid_returns_err() {
    let table = ProcessTable::new();
    let result = table.set_token(Pid::from_u64(99), vec![], None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn increment_chain_depth_counts_up() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(71, "agent")).await;
    table.increment_chain_depth(Pid::from_u64(71)).await.unwrap();
    table.increment_chain_depth(Pid::from_u64(71)).await.unwrap();
    table.increment_chain_depth(Pid::from_u64(71)).await.unwrap();
    let entry = table.get(Pid::from_u64(71)).await.unwrap();
    assert_eq!(entry.tool_chain_depth, 3);
}

#[tokio::test]
async fn reset_chain_depth_sets_to_zero() {
    let table = ProcessTable::new();
    table.insert(make_agent_entry(72, "agent")).await;
    table.increment_chain_depth(Pid::from_u64(72)).await.unwrap();
    table.increment_chain_depth(Pid::from_u64(72)).await.unwrap();
    table.reset_chain_depth(Pid::from_u64(72)).await.unwrap();
    let entry = table.get(Pid::from_u64(72)).await.unwrap();
    assert_eq!(entry.tool_chain_depth, 0);
}

#[tokio::test]
async fn new_entry_has_zero_chain_depth_and_empty_tools() {
    let entry = make_agent_entry(73, "fresh");
    assert_eq!(entry.tool_chain_depth, 0);
    assert!(entry.granted_tools.is_empty());
    assert!(entry.token_expires_at.is_none());
}
