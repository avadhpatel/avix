use avix_core::tool_registry::{ToolEntry, ToolRegistry};
use avix_core::types::tool::{ToolName, ToolState, ToolVisibility};
use std::sync::Arc;
use std::time::Duration;

fn make_entry(name: &str) -> ToolEntry {
    ToolEntry {
        name: ToolName::parse(name).unwrap(),
        owner: "test-svc".into(),
        state: ToolState::Available,
        visibility: ToolVisibility::All,
        descriptor: serde_json::json!({"name": name}),
    }
}

fn user_entry(name: &str, user: &str) -> ToolEntry {
    ToolEntry {
        name: ToolName::parse(name).unwrap(),
        owner: "test-svc".into(),
        state: ToolState::Available,
        visibility: ToolVisibility::User(user.to_string()),
        descriptor: serde_json::json!({"name": name}),
    }
}

#[tokio::test]
async fn registry_add_and_lookup() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    let entry = reg.lookup("fs/read").await.unwrap();
    assert_eq!(entry.name.as_str(), "fs/read");
}

#[tokio::test]
async fn registry_lookup_missing_returns_error() {
    let reg = ToolRegistry::new();
    let err = reg.lookup("fs/nonexistent").await.unwrap_err();
    assert!(err.to_string().contains("tool not found"));
}

#[tokio::test]
async fn registry_add_multiple_tools() {
    let reg = ToolRegistry::new();
    let entries = vec![
        make_entry("fs/read"),
        make_entry("fs/write"),
        make_entry("fs/delete"),
    ];
    reg.add("svc", entries).await.unwrap();
    assert_eq!(reg.tool_count().await, 3);
}

#[tokio::test]
async fn registry_remove_tool() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("fs/read"), make_entry("fs/write")])
        .await
        .unwrap();
    reg.remove("svc", &["fs/write"], "service shutdown", false)
        .await
        .unwrap();
    assert_eq!(reg.tool_count().await, 1);
    assert!(reg.lookup("fs/write").await.is_err());
}

#[tokio::test]
async fn registry_set_state() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    reg.set_state("fs/read", ToolState::Degraded).await.unwrap();
    let entry = reg.lookup("fs/read").await.unwrap();
    assert_eq!(entry.state, ToolState::Degraded);
}

#[tokio::test]
async fn registry_set_state_missing_returns_error() {
    let reg = ToolRegistry::new();
    let err = reg
        .set_state("fs/nonexistent", ToolState::Unavailable)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("tool not found"));
}

#[tokio::test]
async fn registry_add_emits_event() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    let evt = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evt.op, "added");
    assert!(evt.tools.contains(&"fs/read".to_string()));
}

#[tokio::test]
async fn registry_remove_emits_event() {
    let (reg, mut events) = ToolRegistry::new_with_events();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    // Consume the add event
    let _ = tokio::time::timeout(Duration::from_secs(1), events.recv()).await;

    reg.remove("svc", &["fs/read"], "gone", false)
        .await
        .unwrap();
    let evt = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(evt.op.starts_with("removed"));
    assert!(evt.tools.contains(&"fs/read".to_string()));
}

#[tokio::test]
async fn registry_lookup_for_user_all_visibility() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    let entry = reg.lookup_for_user("fs/read", "alice").await.unwrap();
    assert_eq!(entry.name.as_str(), "fs/read");
}

#[tokio::test]
async fn registry_lookup_for_user_user_visibility_match() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![user_entry("private/tool", "alice")])
        .await
        .unwrap();
    assert!(reg.lookup_for_user("private/tool", "alice").await.is_ok());
    assert!(reg.lookup_for_user("private/tool", "bob").await.is_err());
}

#[tokio::test]
async fn registry_acquire_guard_holds_permit() {
    let reg = Arc::new(ToolRegistry::new());
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();
    let _guard = reg.acquire("fs/read").await.unwrap();
    // While guard is held, we can still acquire more (semaphore is large)
    let _guard2 = reg.acquire("fs/read").await.unwrap();
}

#[tokio::test]
async fn registry_drain_waits_for_in_flight() {
    let reg = Arc::new(ToolRegistry::new());
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();

    // Acquire a guard (simulate in-flight call)
    let guard = reg.acquire("fs/read").await.unwrap();

    let reg2 = Arc::clone(&reg);
    // Spawn a task to remove with drain=true — it should wait
    let remove_task = tokio::spawn(async move {
        reg2.remove("svc", &["fs/read"], "shutdown", true)
            .await
            .unwrap();
    });

    // Release after a short time
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(guard);

    // Remove task should complete
    tokio::time::timeout(Duration::from_secs(2), remove_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reg.tool_count().await, 0);
}

#[tokio::test]
async fn registry_lookup_timing_under_5us() {
    let reg = ToolRegistry::new();
    reg.add("svc", vec![make_entry("fs/read")]).await.unwrap();

    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = reg.lookup("fs/read").await.unwrap();
    }
    let avg_ns = start.elapsed().as_nanos() / 1000;
    // Allow 100x headroom in debug builds — target is <5µs in release
    assert!(
        avg_ns < 500_000,
        "lookup took {avg_ns} ns avg, expected < 500µs"
    );
}
