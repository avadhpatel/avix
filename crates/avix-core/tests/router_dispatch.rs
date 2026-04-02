/// Integration tests for RouterDispatcher, mangle/unmangle, and capability checks (Gap B).
use avix_core::{
    ipc::{
        message::{IpcMessage, JsonRpcRequest, JsonRpcResponse},
        IpcServer,
    },
    process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable},
    router::{
        capability::{check_capability, ALWAYS_PRESENT},
        mangle::{mangle, unmangle, validate_tool_name},
        RouterDispatcher, ServiceRegistry,
    },
    tool_registry::{ToolEntry, ToolPermissions, ToolRegistry, ToolState, ToolVisibility},
    types::{tool::ToolName, Pid},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tempfile::tempdir;

// ── helpers ──────────────────────────────────────────────────────────────────

async fn simple_mock_service(sock_path: std::path::PathBuf) -> avix_core::ipc::IpcServerHandle {
    let (server, handle) = IpcServer::bind(sock_path).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(|msg| {
                Box::pin(async move {
                    match msg {
                        IpcMessage::Request(req) => {
                            Some(JsonRpcResponse::ok(&req.id, json!({"pong": true})))
                        }
                        IpcMessage::Notification(_) => None,
                    }
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    handle
}

async fn build_dispatcher(
    sock_path: &std::path::Path,
    tool_name: &str,
    granted: Vec<String>,
) -> (RouterDispatcher, Arc<ProcessTable>) {
    let service_registry = Arc::new(ServiceRegistry::new());
    service_registry
        .register("echo-svc", sock_path.to_str().unwrap())
        .await;
    service_registry.register_tool(tool_name, "echo-svc").await;

    let tool_registry = Arc::new(ToolRegistry::new());
    tool_registry
        .add(
            "echo-svc",
            vec![ToolEntry {
                name: ToolName::parse(tool_name).unwrap(),
                owner: "echo-svc".into(),
                state: ToolState::Available,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
                capabilities_required: vec![],
                permissions: ToolPermissions::default(),
            }],
        )
        .await
        .unwrap();

    let process_table = Arc::new(ProcessTable::new());
    process_table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "agent".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            spawned_by_user: "alice".into(),
            granted_tools: granted,
            ..Default::default()
        })
        .await;

    let dispatcher = RouterDispatcher::new(service_registry, tool_registry, process_table.clone())
        .with_call_timeout(Duration::from_millis(500));

    (dispatcher, process_table)
}

fn make_request(method: &str) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: method.into(),
        params: json!({}),
    }
}

// ── T-B-01: Successful dispatch ───────────────────────────────────────────────

#[tokio::test]
async fn dispatch_routes_to_correct_service() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("echo.sock");
    let _handle = simple_mock_service(sock.clone()).await;

    let (dispatcher, _) = build_dispatcher(&sock, "echo/ping", vec!["echo/ping".into()]).await;

    let resp = dispatcher
        .dispatch(make_request("echo/ping"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.result.is_some(), "expected result, got: {resp:?}");
    assert_eq!(resp.result.unwrap()["pong"], true);
}

// ── T-B-02: Unknown tool returns ENOTFOUND_METHOD (-32601) ───────────────────

#[tokio::test]
async fn dispatch_unknown_tool_returns_not_found() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("echo2.sock");
    let _handle = simple_mock_service(sock.clone()).await;

    let (dispatcher, _) = build_dispatcher(&sock, "echo/ping", vec!["echo/ping".into()]).await;

    let resp = dispatcher
        .dispatch(make_request("ghost/unknown"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32601);
}

// ── T-B-03: Unavailable tool returns EUNAVAIL (-32005) ───────────────────────

#[tokio::test]
async fn dispatch_unavailable_tool_returns_eunavail() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("echo3.sock");
    let _handle = simple_mock_service(sock.clone()).await;

    let service_registry = Arc::new(ServiceRegistry::new());
    service_registry
        .register("svc", sock.to_str().unwrap())
        .await;
    service_registry.register_tool("down/tool", "svc").await;

    let tool_registry = Arc::new(ToolRegistry::new());
    tool_registry
        .add(
            "svc",
            vec![ToolEntry {
                name: ToolName::parse("down/tool").unwrap(),
                owner: "svc".into(),
                state: ToolState::Unavailable,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
                capabilities_required: vec![],
                permissions: ToolPermissions::default(),
            }],
        )
        .await
        .unwrap();

    let process_table = Arc::new(ProcessTable::new());
    process_table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "a".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            spawned_by_user: "alice".into(),
            granted_tools: vec!["down/tool".into()],
            ..Default::default()
        })
        .await;

    let dispatcher = RouterDispatcher::new(service_registry, tool_registry, process_table)
        .with_call_timeout(Duration::from_millis(500));

    let resp = dispatcher
        .dispatch(make_request("down/tool"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32005);
}

// ── T-B-04: Concurrency limit → EBUSY (-32008) ────────────────────────────────

#[tokio::test]
async fn dispatch_at_capacity_returns_ebusy() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("slow.sock");

    // Slow service — holds connection open 200 ms.
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
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let (_dispatcher, _) = build_dispatcher(&sock, "slow/op", vec!["slow/op".into()]).await;

    // Override to max_concurrent=1.
    let service_registry = Arc::new(ServiceRegistry::new());
    service_registry
        .register("echo-svc", sock.to_str().unwrap())
        .await;
    service_registry.register_tool("slow/op", "echo-svc").await;

    let tool_registry = Arc::new(ToolRegistry::new());
    tool_registry
        .add(
            "echo-svc",
            vec![ToolEntry {
                name: ToolName::parse("slow/op").unwrap(),
                owner: "echo-svc".into(),
                state: ToolState::Available,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
                capabilities_required: vec![],
                permissions: ToolPermissions::default(),
            }],
        )
        .await
        .unwrap();

    let process_table = Arc::new(ProcessTable::new());
    process_table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "a".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "alice".into(),
            granted_tools: vec!["slow/op".into()],
            ..Default::default()
        })
        .await;

    let limited = Arc::new(
        RouterDispatcher::new(service_registry, tool_registry, process_table)
            .with_max_concurrent(1)
            .with_call_timeout(Duration::from_millis(500)),
    );

    let lim_clone = limited.clone();

    // First call — occupies the single slot.
    let first = tokio::spawn(async move {
        lim_clone
            .dispatch(make_request("slow/op"), Pid::new(10), "alice", "tok")
            .await
    });

    // Give first call time to enter the dispatcher and acquire the slot.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Second call — should get EBUSY.
    let resp = limited
        .dispatch(make_request("slow/op"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.error.is_some(), "expected EBUSY error, got: {resp:?}");
    assert_eq!(resp.error.unwrap().code, -32008, "expected EBUSY (-32008)");

    first.await.unwrap();
}

// ── T-B-05: Timeout returns ETIMEOUT (-32007) ─────────────────────────────────

#[tokio::test]
async fn dispatch_slow_service_returns_etimeout() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("to.sock");

    let (server, _handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(|msg| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    match msg {
                        IpcMessage::Request(req) => Some(JsonRpcResponse::ok(&req.id, json!({}))),
                        IpcMessage::Notification(_) => None,
                    }
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let service_registry = Arc::new(ServiceRegistry::new());
    service_registry
        .register("echo-svc", sock.to_str().unwrap())
        .await;
    service_registry.register_tool("slow/op", "echo-svc").await;

    let tool_registry = Arc::new(ToolRegistry::new());
    tool_registry
        .add(
            "echo-svc",
            vec![ToolEntry {
                name: ToolName::parse("slow/op").unwrap(),
                owner: "echo-svc".into(),
                state: ToolState::Available,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
                capabilities_required: vec![],
            }],
        )
        .await
        .unwrap();

    let process_table = Arc::new(ProcessTable::new());
    process_table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "a".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "alice".into(),
            granted_tools: vec!["slow/op".into()],
            ..Default::default()
        })
        .await;

    let dispatcher = RouterDispatcher::new(service_registry, tool_registry, process_table)
        .with_call_timeout(Duration::from_millis(50));

    let resp = dispatcher
        .dispatch(make_request("slow/op"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.error.is_some());
    assert_eq!(
        resp.error.unwrap().code,
        -32007,
        "expected ETIMEOUT (-32007)"
    );
}

// ── T-B-06: _caller is injected into forwarded params ────────────────────────

#[tokio::test]
async fn dispatch_injects_caller() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("caller.sock");

    // Service echoes back the full params it received.
    let (server, _handle) = IpcServer::bind(sock.clone()).await.unwrap();
    tokio::spawn(async move {
        server
            .serve(|msg| {
                Box::pin(async move {
                    match msg {
                        IpcMessage::Request(req) => {
                            Some(JsonRpcResponse::ok(&req.id, req.params.clone()))
                        }
                        IpcMessage::Notification(_) => None,
                    }
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = Option<JsonRpcResponse>> + Send>,
                    >
            })
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Build a caller-scoped dispatcher manually so `_caller` is injected.
    let service_registry = Arc::new(ServiceRegistry::new());
    service_registry
        .register_with_meta("echo-svc", sock.to_str().unwrap(), true)
        .await;
    service_registry
        .register_tool("echo/ping", "echo-svc")
        .await;

    let tool_registry = Arc::new(ToolRegistry::new());
    tool_registry
        .add(
            "echo-svc",
            vec![ToolEntry {
                name: ToolName::parse("slow/op").unwrap(),
                owner: "echo-svc".into(),
                state: ToolState::Available,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
                capabilities_required: vec![],
            }],
        )
        .await
        .unwrap();

    let process_table = Arc::new(ProcessTable::new());
    process_table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "agent".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            spawned_by_user: "alice".into(),
            granted_tools: vec!["echo/ping".into()],
            ..Default::default()
        })
        .await;

    let dispatcher = RouterDispatcher::new(service_registry, tool_registry, process_table)
        .with_call_timeout(Duration::from_millis(500));

    let resp = dispatcher
        .dispatch(make_request("echo/ping"), Pid::new(10), "alice", "tok")
        .await;

    let result = resp.result.expect("expected result");
    assert_eq!(result["_caller"]["pid"], 10);
    assert_eq!(result["_caller"]["user"], "alice");
}

// ── T-B-07: Mangle / unmangle ────────────────────────────────────────────────

#[test]
fn mangle_unmangle_round_trip() {
    assert_eq!(mangle("fs/read"), "fs__read");
    assert_eq!(unmangle("fs__read"), "fs/read");
    assert!(validate_tool_name("fs__read").is_err());
    assert!(validate_tool_name("fs/read").is_ok());
}

// ── T-B-08: Capability check blocks unauthorized tool ────────────────────────

#[tokio::test]
async fn capability_check_blocks_unauthorized() {
    let table = Arc::new(ProcessTable::new());
    table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "a".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "alice".into(),
            granted_tools: vec!["fs/read".into()],
            ..Default::default()
        })
        .await;

    check_capability("fs/write", Pid::new(10), &table)
        .await
        .unwrap_err();
    check_capability("fs/read", Pid::new(10), &table)
        .await
        .unwrap();
}

// ── T-B-09: Always-present tools bypass capability check ─────────────────────

#[tokio::test]
async fn always_present_tools_are_always_allowed() {
    let table = Arc::new(ProcessTable::new());
    table
        .insert(ProcessEntry {
            pid: Pid::new(10),
            name: "a".into(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "alice".into(),
            granted_tools: vec![], // empty
            ..Default::default()
        })
        .await;

    for tool in ALWAYS_PRESENT {
        check_capability(tool, Pid::new(10), &table).await.unwrap();
    }
}

// ── T-B-10: Dispatch denied when tool not in granted_tools ───────────────────

#[tokio::test]
async fn dispatch_denied_for_unauthorized_caller() {
    let dir = tempdir().unwrap();
    let sock = dir.path().join("auth.sock");
    let _handle = simple_mock_service(sock.clone()).await;

    // Build dispatcher but grant NO tools.
    let (dispatcher, _) = build_dispatcher(&sock, "echo/ping", vec![]).await;

    let resp = dispatcher
        .dispatch(make_request("echo/ping"), Pid::new(10), "alice", "tok")
        .await;

    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, -32002, "expected EPERM (-32002)");
}
