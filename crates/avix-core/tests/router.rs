use avix_core::router::ServiceRegistry;

#[tokio::test]
async fn register_and_lookup_service() {
    let reg = ServiceRegistry::new();
    reg.register("github-svc", "/run/avix/services/github-svc.sock")
        .await;
    let ep = reg.lookup("github-svc").await.unwrap();
    assert_eq!(ep, "/run/avix/services/github-svc.sock");
}

#[tokio::test]
async fn lookup_unregistered_returns_none() {
    let reg = ServiceRegistry::new();
    assert!(reg.lookup("ghost-svc").await.is_none());
}

#[tokio::test]
async fn deregister_removes_service() {
    let reg = ServiceRegistry::new();
    reg.register("svc", "/run/avix/services/svc.sock").await;
    reg.deregister("svc").await;
    assert!(reg.lookup("svc").await.is_none());
}

#[tokio::test]
async fn route_tool_to_correct_service() {
    let reg = ServiceRegistry::new();
    reg.register_tool("fs/read", "memfs-svc").await;
    reg.register_tool("fs/write", "memfs-svc").await;
    reg.register_tool("llm/complete", "llm-svc").await;
    assert_eq!(reg.service_for_tool("fs/read").await.unwrap(), "memfs-svc");
    assert_eq!(
        reg.service_for_tool("llm/complete").await.unwrap(),
        "llm-svc"
    );
}

#[tokio::test]
async fn route_unknown_tool_returns_none() {
    let reg = ServiceRegistry::new();
    assert!(reg.service_for_tool("ghost/tool").await.is_none());
}

#[test]
fn caller_injected_into_params() {
    use avix_core::router::CallerInfo;
    use serde_json::json;
    let mut params = json!({"path": "/etc/test.yaml"});
    CallerInfo {
        pid: 57,
        user: "alice".into(),
        token: "tok".into(),
    }
    .inject_into(&mut params);
    assert_eq!(params["_caller"]["pid"], 57);
    assert_eq!(params["_caller"]["user"], "alice");
}

#[test]
fn caller_does_not_overwrite_existing_params() {
    use avix_core::router::CallerInfo;
    use serde_json::json;
    let mut params = json!({"path": "/test"});
    CallerInfo {
        pid: 57,
        user: "alice".into(),
        token: "tok".into(),
    }
    .inject_into(&mut params);
    assert_eq!(params["path"], "/test");
}

#[tokio::test]
async fn concurrent_tool_registrations() {
    use std::sync::Arc;
    let reg = Arc::new(ServiceRegistry::new());
    let mut handles = Vec::new();
    for i in 0..50u32 {
        let r = Arc::clone(&reg);
        handles.push(tokio::spawn(async move {
            r.register_tool(&format!("svc/tool-{i}"), "test-svc").await;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(reg.tool_count().await, 50);
}

// Day 10 — concurrency limiter tests
use avix_core::router::concurrency::ConcurrencyLimiter;

#[tokio::test]
async fn concurrent_calls_within_limit_all_proceed() {
    let limiter = ConcurrencyLimiter::new(5);
    let mut guards = Vec::new();
    for _ in 0..5 {
        guards.push(limiter.acquire().await.unwrap());
    }
    assert_eq!(limiter.active_count().await, 5);
    drop(guards);
    assert_eq!(limiter.active_count().await, 0);
}

#[tokio::test]
async fn acquire_beyond_limit_blocks_until_slot_available() {
    use std::time::Duration;
    let limiter = std::sync::Arc::new(ConcurrencyLimiter::new(2));
    let g1 = limiter.acquire().await.unwrap();
    let g2 = limiter.acquire().await.unwrap();
    let lim = std::sync::Arc::clone(&limiter);
    let handle = tokio::spawn(async move { lim.acquire().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(g1);
    tokio::time::timeout(Duration::from_millis(200), handle)
        .await
        .expect("should have acquired within timeout")
        .unwrap();
    drop(g2);
}

#[tokio::test]
async fn caller_scoped_limiter_tracks_per_caller() {
    use avix_core::router::concurrency::CallerScopedLimiter;
    use avix_core::types::Pid;
    let limiter = CallerScopedLimiter::new(2);
    let g1 = limiter.acquire(Pid::from_u64(57)).await.unwrap();
    let g2 = limiter.acquire(Pid::from_u64(57)).await.unwrap();
    let g3 = limiter.acquire(Pid::from_u64(58)).await.unwrap();
    assert!(g3.is_valid());
    drop(g1);
    drop(g2);
    drop(g3);
}

#[test]
fn caller_injected_with_correct_pid_and_user() {
    use avix_core::router::CallerInfo;
    use serde_json::json;
    let mut params = json!({});
    CallerInfo {
        pid: 57,
        user: "alice".into(),
        token: "tok".into(),
    }
    .inject_into(&mut params);
    assert_eq!(params["_caller"]["pid"], 57);
    assert_eq!(params["_caller"]["user"], "alice");
}

#[test]
fn caller_injection_preserves_existing_fields() {
    use avix_core::router::CallerInfo;
    use serde_json::json;
    let mut params = json!({"path": "/test", "content": "hello"});
    CallerInfo {
        pid: 10,
        user: "bob".into(),
        token: "tok".into(),
    }
    .inject_into(&mut params);
    assert_eq!(params["path"], "/test");
    assert_eq!(params["content"], "hello");
}
