/// Integration tests for signal delivery over IPC (Gap C).
use avix_core::{
    executor::{
        runtime_executor::MockToolRegistry,
        spawn::SpawnParams,
        RuntimeExecutor,
    },
    ipc::message::{IpcMessage, JsonRpcResponse},
    signal::{
        agent_socket::{create_agent_socket, remove_agent_socket},
        delivery::SignalDelivery,
        kind::{Signal, SignalKind},
    },
    types::{token::CapabilityToken, Pid},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tempfile::tempdir;

// ── T-C-01: SignalKind includes SIGUSR1 and SIGUSR2 ──────────────────────────

#[test]
fn signal_kind_usr1_usr2_exist() {
    assert_eq!(SignalKind::Usr1.as_str(), "SIGUSR1");
    assert_eq!(SignalKind::Usr2.as_str(), "SIGUSR2");
}

// ── T-C-02: Deliver sends notification to agent socket ────────────────────────

#[tokio::test]
async fn deliver_sends_notification_to_agent_socket() {
    let dir = tempdir().unwrap();

    // Bind agent socket for pid=57 — the delivery path requires agents/57.sock to exist.
    let (server, _handle) =
        create_agent_socket(dir.path(), Pid::new(57)).await.unwrap();

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

// ── T-C-03: Deliver returns NotFound if socket does not exist ─────────────────

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

// ── T-C-04: Broadcast delivers to all listed PIDs ────────────────────────────

#[tokio::test]
async fn broadcast_reaches_multiple_agents() {
    let dir = tempdir().unwrap();
    let pids = [Pid::new(1), Pid::new(2), Pid::new(3)];

    // Bind agent sockets for each pid.
    let mut handles = Vec::new();
    for &pid in &pids {
        let (server, handle) = create_agent_socket(dir.path(), pid).await.unwrap();
        handles.push(handle);
        tokio::spawn(async move {
            server
                .serve(|_| {
                    Box::pin(async move { None })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send,
                            >,
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

// ── T-C-05: Broadcast tolerates missing agents (partial delivery) ─────────────

#[tokio::test]
async fn broadcast_tolerates_missing_agents() {
    let dir = tempdir().unwrap();

    // Only pid=1 has a socket; 2 and 3 do not.
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
    let results = delivery
        .broadcast(&pids, SignalKind::Stop, json!({}))
        .await;

    assert_eq!(results.len(), 3);

    let ok_count = results.iter().filter(|(_, r)| r.is_ok()).count();
    let err_count = results.iter().filter(|(_, r)| r.is_err()).count();
    assert_eq!(ok_count, 1, "expected 1 success");
    assert_eq!(err_count, 2, "expected 2 failures");
}

// ── T-C-06: Agent signal listener handles SIGPAUSE and SIGRESUME ──────────────

#[tokio::test]
async fn agent_pause_resume_via_signal() {
    let dir = tempdir().unwrap();

    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(42),
        agent_name: "signal-test".into(),
        goal: "test".into(),
        spawned_by: "alice".into(),
        session_id: "sess-1".into(),
        token: CapabilityToken::test_token(&[]),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap();

    let paused = executor.paused.clone();
    assert!(!paused.load(std::sync::atomic::Ordering::Acquire), "should not start paused");

    let (_task, server_handle) = executor
        .start_signal_listener(dir.path())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(10)).await;

    // Deliver SIGPAUSE.
    let delivery = SignalDelivery::new(dir.path().to_path_buf());
    delivery
        .deliver(Signal {
            target: Pid::new(42),
            kind: SignalKind::Pause,
            payload: json!({}),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(paused.load(std::sync::atomic::Ordering::Acquire), "should be paused after SIGPAUSE");

    // Deliver SIGRESUME.
    delivery
        .deliver(Signal {
            target: Pid::new(42),
            kind: SignalKind::Resume,
            payload: json!({}),
        })
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!paused.load(std::sync::atomic::Ordering::Acquire), "should be resumed after SIGRESUME");

    server_handle.cancel();
}

// ── T-C-07: Agent signal listener handles SIGKILL ────────────────────────────

#[tokio::test]
async fn agent_kill_sets_killed_flag() {
    let dir = tempdir().unwrap();

    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::new(43),
        agent_name: "kill-test".into(),
        goal: "test".into(),
        spawned_by: "alice".into(),
        session_id: "sess-2".into(),
        token: CapabilityToken::test_token(&[]),
    };
    let executor = RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap();

    let killed = executor.killed.clone();
    assert!(!killed.load(std::sync::atomic::Ordering::Acquire));

    let (_task, server_handle) = executor
        .start_signal_listener(dir.path())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(10)).await;

    SignalDelivery::new(dir.path().to_path_buf())
        .deliver(Signal {
            target: Pid::new(43),
            kind: SignalKind::Kill,
            payload: json!({}),
        })
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            if killed.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("killed flag not set within 200ms");

    server_handle.cancel();
}

// ── T-C-08: Agent socket is created and removed ───────────────────────────────

#[tokio::test]
async fn agent_socket_created_and_removed() {
    let dir = tempdir().unwrap();

    // Socket must not exist before creation.
    let agents_dir = dir.path().join("agents");
    let sock = agents_dir.join("42.sock");
    assert!(!sock.exists());

    let (server, handle) = create_agent_socket(dir.path(), Pid::new(42))
        .await
        .unwrap();
    assert!(sock.exists(), "socket should exist after bind");

    // Serve briefly then cancel.
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

    // Remove the socket explicitly.
    remove_agent_socket(dir.path(), Pid::new(42)).await.unwrap();
    assert!(!sock.exists(), "socket should be removed");
}

// ── T-C-09: remove_agent_socket is a no-op when socket does not exist ─────────

#[tokio::test]
async fn remove_agent_socket_noop_when_missing() {
    let dir = tempdir().unwrap();
    // Should not error even if the socket was never created.
    remove_agent_socket(dir.path(), Pid::new(99)).await.unwrap();
}
