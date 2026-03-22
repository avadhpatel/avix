/// Integration tests for IpcServer and IpcClient (Gap A).
///
/// All tests use a tempdir so they never touch /run/avix.
use avix_core::ipc::{
    message::{IpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
    IpcClient, IpcServer,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::tempdir;

fn echo_handler(
    msg: IpcMessage,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>> {
    Box::pin(async move {
        match msg {
            IpcMessage::Request(req) => {
                Some(JsonRpcResponse::ok(&req.id, json!({"echo": req.method})))
            }
            IpcMessage::Notification(_) => None,
        }
    })
}

/// T-A-01: Server binds and responds to a single call.
#[tokio::test]
async fn server_accepts_single_call() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("test.sock");

    let (server, handle) = IpcServer::bind(sock.clone()).await.unwrap();
    let sock_clone = sock.clone();
    tokio::spawn(async move {
        server.serve(echo_handler).await.unwrap();
    });

    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(10)).await;

    let client = IpcClient::new(sock_clone);
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "test/ping".into(),
        params: json!({}),
    };
    let resp = client.call(req).await.unwrap();
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["echo"], "test/ping");

    handle.cancel();
}

/// T-A-02: Server handles concurrent calls independently.
#[tokio::test]
async fn server_handles_concurrent_calls() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("concurrent.sock");

    let (server, handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server.serve(echo_handler).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    let mut tasks = Vec::new();
    for i in 0..10u32 {
        let s = sock.clone();
        tasks.push(tokio::spawn(async move {
            let client = IpcClient::new(s);
            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: i.to_string(),
                method: format!("test/method-{i}"),
                params: json!({}),
            };
            client.call(req).await.unwrap()
        }));
    }

    let results: Vec<JsonRpcResponse> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(results.len(), 10);
    for r in &results {
        assert!(r.result.is_some());
    }

    handle.cancel();
}

/// T-A-03: Notification (no id) receives no response; connection closes cleanly.
#[tokio::test]
async fn server_ignores_notification_response() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("notif.sock");

    let received = Arc::new(Mutex::new(false));
    let received_clone = received.clone();

    let (server, handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(move |msg| {
                let flag = received_clone.clone();
                Box::pin(async move {
                    if let IpcMessage::Notification(_) = msg {
                        *flag.lock().unwrap() = true;
                    }
                    None
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>>
            })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    let client = IpcClient::new(sock.clone());
    let notif = JsonRpcNotification::new("signal", json!({"signal": "SIGPAUSE", "payload": {}}));
    client.notify(notif).await.unwrap();

    // Allow the server task to process the notification.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(*received.lock().unwrap());

    handle.cancel();
}

/// T-A-04: Client times out when the server handler is slow.
#[tokio::test]
async fn client_timeout_on_slow_server() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("slow.sock");

    let (server, _handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(|msg| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    match msg {
                        IpcMessage::Request(req) => {
                            Some(JsonRpcResponse::ok(&req.id, json!({"ok": true})))
                        }
                        IpcMessage::Notification(_) => None,
                    }
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>>
            })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    let client = IpcClient::new(sock.clone()).with_timeout(Duration::from_millis(50));
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "x".into(),
        method: "slow/op".into(),
        params: json!({}),
    };
    let result = client.call(req).await;
    assert!(
        matches!(result, Err(avix_core::error::AvixError::IpcTimeout)),
        "expected IpcTimeout, got: {result:?}"
    );
}

/// T-A-05: Client uses a fresh connection per call (second call works after server rebind).
#[tokio::test]
async fn client_fresh_connection_per_call() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("fresh.sock");

    // First server binding.
    let (server, handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server.serve(echo_handler).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let client = IpcClient::new(sock.clone());
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "a".into(),
        method: "ping".into(),
        params: json!({}),
    };
    let resp = client.call(req).await.unwrap();
    assert!(resp.result.is_some());

    // Cancel first server; rebind on same path.
    handle.cancel();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (server2, handle2) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server2.serve(echo_handler).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let req2 = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "b".into(),
        method: "ping2".into(),
        params: json!({}),
    };
    let resp2 = client.call(req2).await.unwrap();
    assert!(resp2.result.is_some());

    handle2.cancel();
}

/// T-A-06: Server drains in-flight calls before returning from serve().
#[tokio::test]
async fn server_graceful_shutdown_drains_inflight() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("drain.sock");

    let completed = Arc::new(Mutex::new(false));
    let completed_clone = completed.clone();

    let (server, handle) = IpcServer::bind(sock.clone()).await.unwrap();
    let serve_task = tokio::spawn(async move {
        server
            .serve(move |msg| {
                let flag = completed_clone.clone();
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    *flag.lock().unwrap() = true;
                    match msg {
                        IpcMessage::Request(req) => {
                            Some(JsonRpcResponse::ok(&req.id, json!({"done": true})))
                        }
                        IpcMessage::Notification(_) => None,
                    }
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>>
            })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(10)).await;

    // Start a call that will take 80ms.
    let client = IpcClient::new(sock.clone()).with_timeout(Duration::from_millis(500));
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "drain".into(),
        method: "slow".into(),
        params: json!({}),
    };
    let call_task = tokio::spawn(async move { client.call(req).await });

    // Cancel the server while the call is in-flight.
    tokio::time::sleep(Duration::from_millis(20)).await;
    handle.cancel();

    // serve() should not return until the in-flight call completes.
    tokio::time::timeout(Duration::from_millis(300), serve_task)
        .await
        .expect("serve did not finish within 300ms")
        .unwrap();

    assert!(*completed.lock().unwrap(), "in-flight call was not completed before shutdown");
    call_task.await.unwrap().unwrap();
}

/// T-A-07: Client returns an error when connecting to a non-existent socket.
#[tokio::test]
async fn client_connect_fails_for_missing_socket() {
    let client = IpcClient::new("/tmp/avix-does-not-exist-xyz.sock".into());
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "test".into(),
        params: json!({}),
    };
    let result = client.call(req).await;
    assert!(
        matches!(result, Err(avix_core::error::AvixError::Io(_))),
        "expected Io error, got: {result:?}"
    );
}
