//! ATP WebSocket Integration Tests
//!
//! Validates end-to-end ATP protocol over WebSocket, including server spawn,
//! authentication, connection, and basic RPC calls.
//!
//! References:
//! - ATP Protocol: docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md
//! - Test Spec: docs/dev_plans/ATP-WS-TEST-SUITE-SPEC-20241025.md

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, span, Level};

/// Spawns a debug Avix server on a dynamic port with temp root.
/// Initializes config with test user, waits for the server to be ready.
/// Returns the child process, port, and API key.
///
/// # ATP Server Startup
/// Refer to docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md for server modes.
async fn spawn_debug_server() -> Result<(tokio::process::Child, u16, String)> {
    let _span = span!(Level::INFO, "spawn_debug_server").entered();
    let port = portpicker::pick_unused_port().expect("No free port");
    let root = tempfile::tempdir()?.path().to_string_lossy().to_string();

    info!("Initializing config for root {}", root);

    // Run config init to create auth.conf with test user
    let init_output = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("avix-cli")
        .arg("--bin")
        .arg("avix")
        .arg("config")
        .arg("init")
        .arg("--root")
        .arg(&root)
        .arg("--user")
        .arg("test")
        .arg("--role")
        .arg("admin")
        .env("AVIX_MASTER_KEY", "changeme")
        .output()
        .await?;
    if !init_output.status.success() {
        return Err(anyhow::anyhow!("Config init failed: {:?}", init_output));
    }
    let init_stdout = String::from_utf8(init_output.stdout)?;
    let api_key = init_stdout
        .lines()
        .find(|line| line.contains("API key (Avix):"))
        .and_then(|line| line.split(": ").nth(1))
        .ok_or_else(|| anyhow::anyhow!("Could not find API key in init output"))?
        .to_string();

    info!("Config initialized, API key: {}", api_key);

    info!("Spawning server on port {} with root {}", port, root);

    let child = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("avix-cli")
        .arg("--bin")
        .arg("avix")
        .arg("--")
        .arg("--log")
        .arg("trace")
        .arg("server")
        .arg("--root")
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
        .arg("--test-mode")
        .env("RUST_LOG", "avix=trace")
        .env("AVIX_MASTER_KEY", "changeme")
        .spawn()?;

    // Wait for port to be open, timeout 30s
    wait_for_port(port).await?;

    info!("Server ready on port {}", port);
    Ok((child, port, api_key))
}

/// Waits for a TCP port to become available.
/// Polls every 500ms for up to 30 seconds.
async fn wait_for_port(port: u16) -> Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    timeout(Duration::from_secs(30), async {
        loop {
            if TcpStream::connect(&addr).await.is_ok() {
                return;
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await?;
    Ok(())
}

/// Logs into the ATP server via HTTP and extracts the token.
/// POST /atp/auth/login with identity/credential JSON.
/// Returns the token string.
///
/// # Authentication
/// Refer to docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md for login flow.
async fn login_http(port: u16, credential: &str) -> Result<String> {
    let _span = span!(Level::INFO, "login_http", port).entered();
    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/atp/auth/login", port);
    let resp = client
        .post(&url)
        .json(&json!({"identity": "test", "credential": credential}))
        .send()
        .await?;
    let body: Value = resp.json().await?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No token in response: {:?}", body))?
        .to_string();
    info!("Logged in, got token");
    Ok(token)
}

/// Connects to the ATP WebSocket with Bearer token.
/// Returns the WebSocket stream split into write/read.
///
/// # WebSocket Connection
/// Refer to docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md for WS upgrade.
async fn ws_connect(
    token: &str,
    port: u16,
) -> Result<(
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
)> {
    let _span = span!(Level::INFO, "ws_connect", port).entered();
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let url = format!("ws://localhost:{}/atp", port);
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {}", token).parse()?);

    let (ws_stream, _) = connect_async(request).await?;
    let (write, read) = ws_stream.split();
    info!("WebSocket connected");
    Ok((write, read))
}

/// Sends a JSON-RPC request over WebSocket and receives the response.
/// Times out after 30 seconds.
///
/// # JSON-RPC
/// ATP supports JSON-RPC 2.0 for commands.
/// Refer to docs/architecture/04-atp.md for RPC details.
async fn send_rpc(
    write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    method: &str,
    params: Value,
) -> Result<Value> {
    let _span = span!(Level::INFO, "send_rpc", method).entered();
    static mut ID: u64 = 0;
    unsafe { ID += 1 };
    let id = unsafe { ID };
    let req = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });
    write.send(Message::Text(req.to_string())).await?;
    // Read response
    let msg_opt = timeout(Duration::from_secs(30), read.next()).await?;
    let msg = msg_opt.ok_or_else(|| anyhow::anyhow!("No response message"))?;
    let msg = msg?;
    match msg {
        Message::Text(text) => {
            let resp: Value = serde_json::from_str(&text)?;
            Ok(resp)
        }
        _ => Err(anyhow::anyhow!("Unexpected message type")),
    }
}

/// Sends a subscribe message over WebSocket.
/// Subscribes to the given events.
/// Refer to ATP protocol for subscription details.
/// Links: docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md
async fn send_subscribe(
    write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    events: Vec<&str>,
) -> Result<()> {
    let _span = span!(Level::INFO, "send_subscribe").entered();
    let req = json!({
        "type": "subscribe",
        "events": events
    });
    write.send(Message::Text(req.to_string())).await?;
    info!("Subscribed to events: {:?}", events);
    Ok(())
}

/// Receives the next event message from WebSocket.
/// Times out after 30 seconds.
/// Returns the parsed JSON Value.
/// Refer to ATP event format.
/// Links: docs/architecture/04-atp.md
async fn recv_event(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<Value> {
    let _span = span!(Level::INFO, "recv_event").entered();
    let msg_opt = timeout(Duration::from_secs(30), read.next()).await?;
    let msg = msg_opt.ok_or_else(|| anyhow::anyhow!("No event message"))?;
    let msg = msg?;
    match msg {
        Message::Text(text) => {
            let event: Value = serde_json::from_str(&text)?;
            Ok(event)
        }
        _ => Err(anyhow::anyhow!("Unexpected message type")),
    }
}

/// Basic ATP test: spawn server, login, connect WS, send session.ready, assert ack, send proc.list, assert array, kill server.
#[tokio::test]
async fn test_basic() -> Result<()> {
    let _span = span!(Level::INFO, "test_basic").entered();
    // Spawn server
    let (mut server, port, api_key) = spawn_debug_server().await?;
    // Login
    let token = login_http(port, &api_key).await?;
    // Connect WS
    let (mut write, mut read) = ws_connect(&token, port).await?;
    // Send session.ready
    let resp = send_rpc(&mut write, &mut read, "session.ready", json!({})).await?;
    assert_eq!(resp["result"], "ack");
    // Send proc.list
    let resp = send_rpc(&mut write, &mut read, "proc.list", json!({})).await?;
    assert!(resp["result"].is_array());
    // Kill server
    server.kill().await?;
    info!("Test passed");
    Ok(())
}

/// Test error handling: invalid token, bad method.
#[tokio::test]
async fn test_errors() -> Result<()> {
    let _span = span!(Level::INFO, "test_errors").entered();
    // Spawn server
    let (mut server, port, _api_key) = spawn_debug_server().await?;
    // Try login with bad credential
    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/atp/auth/login", port);
    let resp = client
        .post(&url)
        .json(&json!({"identity": "test", "credential": "bad"}))
        .send()
        .await?;
    assert_eq!(resp.status(), 401); // Assuming it returns 401
                                    // Try WS with bad token
    let bad_token = "bad";
    let (mut write, mut read) = ws_connect(bad_token, port).await?;
    // Send session.ready, should fail
    let resp = send_rpc(&mut write, &mut read, "session.ready", json!({})).await;
    assert!(resp.is_err()); // Should fail
                            // Kill server
    server.kill().await?;
    info!("Test errors passed");
    Ok(())
}

/// Test events: proc.start and receive event.
#[tokio::test]
async fn test_events() -> Result<()> {
    let _span = span!(Level::INFO, "test_events").entered();
    // Spawn server
    let (mut server, port, api_key) = spawn_debug_server().await?;
    // Login
    let token = login_http(port, &api_key).await?;
    // Connect WS
    let (mut write, mut read) = ws_connect(&token, port).await?;
    // Subscribe to proc events
    send_subscribe(&mut write, vec!["*"]).await?;
    // Send proc.start
    let resp = send_rpc(
        &mut write,
        &mut read,
        "proc.spawn",
        json!({"cmd": ["echo", "hello"], "name": "test-proc"}),
    )
    .await?;
    assert!(resp["result"].is_object()); // Should return proc info
                                         // Receive proc.start event
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "proc.start");
    // Kill server
    server.kill().await?;
    info!("Test events passed");
    Ok(())
}

/// Test full proc lifecycle events: spawn, output, exit, list.
/// Covers Gap4.1: Proc Lifecycle Events (spawn/output/exit).
/// Links: docs/dev_plans/ATP-WS-TESTS-PLAN.md#41
#[tokio::test]
async fn test_proc_lifecycle() -> Result<()> {
    let _span = span!(Level::INFO, "test_proc_lifecycle").entered();
    // Spawn server
    let (mut server, port, api_key) = spawn_debug_server().await?;
    // Login
    let token = login_http(port, &api_key).await?;
    // Connect WS
    let (mut write, mut read) = ws_connect(&token, port).await?;
    // Subscribe to all events
    send_subscribe(&mut write, vec!["*"]).await?;
    // Spawn a proc
    let resp = send_rpc(
        &mut write,
        &mut read,
        "proc.spawn",
        json!({"cmd": ["echo", "hi"], "name": "test-proc"}),
    )
    .await?;
    assert!(resp["result"].is_object());
    let pid = resp["result"]["id"].as_u64().unwrap();
    // Receive proc.start event
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "proc.start");
    assert_eq!(event["pid"], pid);
    // Check proc.list includes it
    let resp = send_rpc(&mut write, &mut read, "proc.list", json!({})).await?;
    let list = resp["result"].as_array().unwrap();
    assert!(list.iter().any(|p| p["id"] == pid));
    // Receive proc.output event (echo outputs)
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "proc.output");
    assert_eq!(event["pid"], pid);
    // Check proc.status
    let resp = send_rpc(&mut write, &mut read, "proc.status", json!({"id": pid})).await?;
    assert!(resp["result"].is_object());
    // Kill the proc
    send_rpc(&mut write, &mut read, "proc.kill", json!({"id": pid})).await?;
    // Receive proc.exit event
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "proc.exit");
    assert_eq!(event["pid"], pid);
    // Check proc.list is empty
    let resp = send_rpc(&mut write, &mut read, "proc.list", json!({})).await?;
    let list = resp["result"].as_array().unwrap();
    assert!(list.is_empty());
    // Kill server
    server.kill().await?;
    info!("Test proc lifecycle passed");
    Ok(())
}

/// Test full errors: bad token (fails on session.ready), invalid method, malformed JSON-RPC.
/// Covers Gap4.2: Full Errors.
/// Links: docs/dev_plans/ATP-WS-TESTS-PLAN.md#42
#[tokio::test]
async fn test_full_errors() -> Result<()> {
    let _span = span!(Level::INFO, "test_full_errors").entered();
    // Spawn server
    let (mut server, port, _api_key) = spawn_debug_server().await?;
    // WS upgrade with bad token succeeds, but session.ready fails
    let (mut write, mut read) = ws_connect("bad-token", port).await?;
    let resp = send_rpc(&mut write, &mut read, "session.ready", json!({})).await;
    assert!(resp.is_err(), "session.ready with bad token should fail");
    // Login to get valid token
    let (mut server2, port2, api_key) = spawn_debug_server().await?;
    let token = login_http(port2, &api_key).await?;
    // Connect WS
    let (mut write, mut read) = ws_connect(&token, port2).await?;
    // Send invalid method
    let resp = send_rpc(&mut write, &mut read, "proc.unknown_op", json!({})).await;
    assert!(resp.is_err(), "Invalid method should error");
    // Send malformed JSON-RPC (missing jsonrpc)
    let malformed = json!({"id": 1, "method": "proc.list", "params": {}});
    write.send(Message::Text(malformed.to_string())).await?;
    let msg_opt = timeout(Duration::from_secs(5), read.next()).await?;
    if let Some(msg) = msg_opt {
        let msg = msg?;
        if let Message::Text(text) = msg {
            let resp: Value = serde_json::from_str(&text)?;
            assert!(
                resp.get("error").is_some(),
                "Malformed JSON should return error"
            );
        }
    }
    // Kill servers
    server.kill().await?;
    server2.kill().await?;
    info!("Test full errors passed");
    Ok(())
}

/// Test basic reconnect: close WS, reopen with same token, session persists.
/// Covers Gap4.3: Basic Reconnect.
/// Links: docs/dev_plans/ATP-WS-TESTS-PLAN.md#43
#[tokio::test]
async fn test_basic_reconnect() -> Result<()> {
    let _span = span!(Level::INFO, "test_basic_reconnect").entered();
    // Spawn server
    let (mut server, port, api_key) = spawn_debug_server().await?;
    // Login
    let token = login_http(port, &api_key).await?;
    // Connect WS first time
    let (mut write, mut read) = ws_connect(&token, port).await?;
    // Subscribe
    send_subscribe(&mut write, vec!["*"]).await?;
    // Spawn a proc
    let resp = send_rpc(
        &mut write,
        &mut read,
        "proc.spawn",
        json!({"cmd": ["sleep", "10"], "name": "persistent-proc"}),
    )
    .await?;
    let pid = resp["result"]["id"].as_u64().unwrap();
    // Close WS (drop handles)
    drop(write);
    drop(read);
    // Reconnect with same token
    let (mut write2, mut read2) = ws_connect(&token, port).await?;
    // Subscribe again
    send_subscribe(&mut write2, vec!["*"]).await?;
    // Check session.ready (should be sent after reconnect)
    let event = recv_event(&mut read2).await?;
    assert_eq!(event["type"], "session.ready");
    // Check proc.list has the same proc
    let resp = send_rpc(&mut write2, &mut read2, "proc.list", json!({})).await?;
    let list = resp["result"].as_array().unwrap();
    assert!(list.iter().any(|p| p["id"] == pid));
    // Kill the proc
    send_rpc(&mut write2, &mut read2, "proc.kill", json!({"id": pid})).await?;
    // Kill server
    server.kill().await?;
    info!("Test basic reconnect passed");
    Ok(())
}

/// Test agent spawn lifecycle: spawn agent → list → kill → verify.
/// Covers PROJECT-SPAWN-001 G8: E2E tests (spawn→list→kill→restart verify).
/// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md
#[tokio::test]
async fn test_agent_spawn_lifecycle() -> Result<()> {
    let _span = span!(Level::INFO, "test_agent_spawn_lifecycle").entered();
    // Spawn server
    let (mut server, port, api_key) = spawn_debug_server().await?;
    // Login
    let token = login_http(port, &api_key).await?;
    // Connect WS
    let (mut write, mut read) = ws_connect(&token, port).await?;
    // Subscribe to all events
    send_subscribe(&mut write, vec!["*"]).await?;
    // Spawn an agent
    let resp = send_rpc(
        &mut write,
        &mut read,
        "proc.spawn",
        json!({"agent": "test-agent", "goal": "Say hello world"}),
    )
    .await?;
    assert!(resp["result"].is_object());
    let pid = resp["result"]["id"].as_u64().unwrap() as u32;
    // Receive agent.spawned event
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "agent.spawned");
    assert_eq!(event["pid"], pid);
    // Check proc.list includes the agent
    let resp = send_rpc(&mut write, &mut read, "proc.list", json!({})).await?;
    let list = resp["result"].as_array().unwrap();
    assert!(list.iter().any(|a| a["id"] == pid));
    let agent = list.iter().find(|a| a["id"] == pid).unwrap();
    assert_eq!(agent["name"], "test-agent");
    assert_eq!(agent["goal"], "Say hello world");
    // Kill the agent
    send_rpc(&mut write, &mut read, "proc.kill", json!({"id": pid})).await?;
    // Receive agent.exit event
    let event = recv_event(&mut read).await?;
    assert_eq!(event["type"], "agent.exit");
    assert_eq!(event["pid"], pid);
    // Check proc.list is empty
    let resp = send_rpc(&mut write, &mut read, "proc.list", json!({})).await?;
    let list = resp["result"].as_array().unwrap();
    assert!(list.is_empty());
    // Kill server
    server.kill().await?;
    info!("Test agent spawn lifecycle passed");
    Ok(())
}
