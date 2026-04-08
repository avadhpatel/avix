/// Integration tests for signal delivery.
///
/// Socket-based delivery tests (T-C-02 through T-C-09) have been superseded by the
/// in-process channel approach.  T-C-06/T-C-07 are rewritten below to use
/// `deliver_signal` directly.  The `agent_socket` and `SignalDelivery` infrastructure
/// is retained only for the pipe-manager path; it is no longer used for executor signals.
use avix_core::{
    executor::{runtime_executor::MockToolRegistry, spawn::SpawnParams, RuntimeExecutor},
    signal::{
        agent_socket::{create_agent_socket, remove_agent_socket},
        delivery::SignalDelivery,
        kind::{Signal, SignalKind},
        pipe_payload::SigPipePayload,
    },
    ipc::message::JsonRpcResponse,
    types::{token::CapabilityToken, Pid},
};
use serde_json::json;
use std::{sync::Arc, sync::atomic::Ordering, time::Duration};
use tempfile::tempdir;

// ── T-C-01: SignalKind includes SIGUSR1 and SIGUSR2 ──────────────────────────

#[test]
fn signal_kind_usr1_usr2_exist() {
    assert_eq!(SignalKind::Usr1.as_str(), "SIGUSR1");
    assert_eq!(SignalKind::Usr2.as_str(), "SIGUSR2");
}

// ── helper ──────────────────────────────────────────────────────────────────

async fn make_executor(pid: u32) -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(pid),
        agent_name: "signal-test".into(),
        goal: "test".into(),
        spawned_by: "alice".into(),
        session_id: "sess-1".into(),
        token: CapabilityToken::test_token(&[]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
    };
    RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
}

// ── T-C-06 (rewritten): deliver_signal SIGPAUSE sets paused flag ──────────

#[tokio::test]
async fn deliver_signal_pause_sets_paused_flag() {
    let executor = make_executor(42).await;
    assert!(!executor.paused.load(Ordering::Acquire));

    executor.deliver_signal("SIGPAUSE").await;

    assert!(executor.paused.load(Ordering::Acquire));
}

// ── T-C-07 (rewritten): deliver_signal SIGRESUME clears paused flag ──────

#[tokio::test]
async fn deliver_signal_resume_clears_paused_flag() {
    let executor = make_executor(43).await;
    executor.deliver_signal("SIGPAUSE").await;
    assert!(executor.paused.load(Ordering::Acquire));

    executor.deliver_signal("SIGRESUME").await;

    assert!(!executor.paused.load(Ordering::Acquire));
}

// ── T-C-08 (rewritten): deliver_signal SIGKILL sets killed flag ──────────

#[tokio::test]
async fn deliver_signal_kill_sets_killed_flag() {
    let executor = make_executor(44).await;
    assert!(!executor.killed.load(Ordering::Acquire));

    executor.deliver_signal("SIGKILL").await;

    assert!(executor.killed.load(Ordering::Acquire));
}

// ── T-C-02: SignalDelivery still works for the pipe-manager path (socket) ──

#[tokio::test]
async fn deliver_sends_notification_to_socket() {
    use avix_core::ipc::message::IpcMessage;

    let dir = tempdir().unwrap();
    let (server, _handle) = create_agent_socket(dir.path(), Pid::new(57)).await.unwrap();

    let received_signal = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let received_clone = received_signal.clone();

    tokio::spawn(async move {
        server
            .serve(move |msg| {
                let flag = received_clone.clone();
                Box::pin(async move {
                    if let IpcMessage::Notification(n) = msg {
                        let name = n.params["signal"].as_str().unwrap_or("").to_string();
                        *flag.lock().await = Some(name);
                    }
                    None
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .ok();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    let delivery = SignalDelivery::new(dir.path().to_path_buf());
    let sig = Signal {
        target: Pid::new(57),
        kind: SignalKind::Pause,
        payload: json!({}),
    };
    delivery.deliver(sig).await.unwrap();

    tokio::time::sleep(Duration::from_millis(20)).await;
    let received = received_signal.lock().await;
    assert_eq!(received.as_deref(), Some("SIGPAUSE"));
}

// ── T-C-03: SignalDelivery returns NotFound if socket does not exist ─────

#[tokio::test]
async fn deliver_returns_not_found_for_missing_agent() {
    let dir = tempdir().unwrap();
    let delivery = SignalDelivery::new(dir.path().to_path_buf());
    let sig = Signal {
        target: Pid::new(99),
        kind: SignalKind::Kill,
        payload: json!({}),
    };
    let result = delivery.deliver(sig).await;
    assert!(
        matches!(result, Err(avix_core::error::AvixError::NotFound(_))),
        "expected NotFound, got: {result:?}"
    );
}

// ── T-C-04: SignalDelivery broadcast delivers to all listed PIDs ─────────

#[tokio::test]
async fn broadcast_reaches_multiple_agents() {
    let dir = tempdir().unwrap();
    let pids = [Pid::new(1), Pid::new(2), Pid::new(3)];

    let mut handles = Vec::new();
    for &pid in &pids {
        let (server, handle) = create_agent_socket(dir.path(), pid).await.unwrap();
        handles.push(handle);
        tokio::spawn(async move {
            server
                .serve(|_| {
                    Box::pin(async move { None })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                        >
                })
                .await
                .ok();
        });
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

    let delivery = SignalDelivery::new(dir.path().to_path_buf());
    let results = delivery
        .broadcast(&pids, SignalKind::Pause, json!({}))
        .await;

    assert_eq!(results.len(), 3);
    for (pid, result) in &results {
        assert!(result.is_ok(), "pid {pid} failed: {result:?}");
    }
}

// ── T-C-05: broadcast tolerates missing agents ───────────────────────────

#[tokio::test]
async fn broadcast_tolerates_missing_agents() {
    let dir = tempdir().unwrap();
    let (server, _handle) = create_agent_socket(dir.path(), Pid::new(1)).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(|_| {
                Box::pin(async move { None })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .ok();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let pids = [Pid::new(1), Pid::new(2), Pid::new(3)];
    let delivery = SignalDelivery::new(dir.path().to_path_buf());
    let results = delivery.broadcast(&pids, SignalKind::Stop, json!({})).await;

    assert_eq!(results.len(), 3);
    let ok_count = results.iter().filter(|(_, r)| r.is_ok()).count();
    let err_count = results.iter().filter(|(_, r)| r.is_err()).count();
    assert_eq!(ok_count, 1);
    assert_eq!(err_count, 2);
}

// ── T-C-09: agent socket created and removed ─────────────────────────────

#[tokio::test]
async fn agent_socket_created_and_removed() {
    let dir = tempdir().unwrap();
    let agents_dir = dir.path().join("agents");
    let sock = agents_dir.join("42.sock");
    assert!(!sock.exists());

    let (server, handle) = create_agent_socket(dir.path(), Pid::new(42)).await.unwrap();
    assert!(sock.exists());

    let serve_task = tokio::spawn(async move {
        server
            .serve(|_| {
                Box::pin(async move { None })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .ok();
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    handle.cancel();
    serve_task.await.unwrap();

    remove_agent_socket(dir.path(), Pid::new(42)).await.unwrap();
    assert!(!sock.exists());
}

// ── T-C-10: remove_agent_socket is a no-op when socket does not exist ────

#[tokio::test]
async fn remove_agent_socket_noop_when_missing() {
    let dir = tempdir().unwrap();
    remove_agent_socket(dir.path(), Pid::new(99)).await.unwrap();
}
