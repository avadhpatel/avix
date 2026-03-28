//! ATP WebSocket Integration Tests
//!
//! Validates end-to-end ATP protocol over WebSocket, including server spawn,
//! authentication, connection, and basic RPC calls.
//!
//! Protocol references:
//! - ATP cmd frame:   {"type":"cmd","id":...,"token":...,"domain":...,"op":...,"body":{}}
//! - ATP reply frame: {"type":"reply","id":...,"ok":bool,"body":...}
//! - ATP event frame: {"type":"event","event":"<kind>","sessionId":...,"ts":...,"body":{}}
//! - ATP subscribe:   {"type":"subscribe","events":[...]}

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, span, Level};

// ── Server lifecycle helpers ───────────────────────────────────────────────────

async fn spawn_debug_server() -> Result<(tokio::process::Child, u16, String)> {
    let _span = span!(Level::INFO, "spawn_debug_server").entered();
    let port = portpicker::pick_unused_port().expect("No free port");
    let root = tempfile::tempdir()?.path().to_string_lossy().to_string();

    info!("Initializing config for root {}", root);
    let init_output = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("avix-cli")
        .arg("--bin")
        .arg("avix")
        .arg("server")
        .arg("config")
        .arg("init")
        .arg("--root")
        .arg(&root)
        .arg("--user")
        .arg("test")
        .arg("--role")
        .arg("admin")
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
        .arg("start")
        .arg("--root")
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
        .arg("--test-mode")
        .env("RUST_LOG", "avix=trace")
        .spawn()?;

    wait_for_port(port).await?;
    info!("Server ready on port {}", port);
    Ok((child, port, api_key))
}

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

/// POST /atp/auth/login and return the JWT token.
async fn login_http(port: u16, credential: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/atp/auth/login", port);
    let resp = client
        .post(&url)
        .json(&json!({"identity": "test", "credential": credential}))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("HTTP error: {}", resp.status()));
    }
    let body: Value = resp.json().await?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No token in response: {:?}", body))?
        .to_string();
    Ok(token)
}

// ── WsConn: ATP-aware WebSocket session ──────────────────────────────────────

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

struct WsConn {
    write: WsSink,
    read: WsStream,
    /// Events received while waiting for a reply.
    event_buf: Vec<Value>,
    token: String,
    id: u64,
}

impl WsConn {
    /// Connect with a Bearer token. Returns Err if the upgrade is rejected.
    async fn connect(token: &str, port: u16) -> Result<Self> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let url = format!("ws://localhost:{}/atp", port);
        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {}", token).parse()?);
        let (ws_stream, _) = connect_async(request).await?;
        let (write, read) = ws_stream.split();
        Ok(Self {
            write,
            read,
            event_buf: Vec::new(),
            token: token.to_string(),
            id: 0,
        })
    }

    /// Send an ATP cmd and wait for the matching reply (buffering any events received first).
    async fn cmd(&mut self, domain: &str, op: &str, body: Value) -> Result<Value> {
        self.id += 1;
        let id = self.id.to_string();
        let frame = json!({
            "type": "cmd",
            "id": id,
            "token": self.token,
            "domain": domain,
            "op": op,
            "body": body,
        });
        self.write
            .send(Message::Text(frame.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("WS send error: {e}"))?;

        loop {
            let msg = timeout(Duration::from_secs(30), self.read.next())
                .await
                .map_err(|_| anyhow::anyhow!("timeout waiting for reply"))?
                .ok_or_else(|| anyhow::anyhow!("WS stream closed"))?
                .map_err(|e| anyhow::anyhow!("WS recv error: {e}"))?;
            let text = match msg {
                Message::Text(t) => t,
                _ => continue,
            };
            let v: Value = serde_json::from_str(&text)?;
            if v["type"] == "reply" && v["id"] == Value::String(id.clone()) {
                return Ok(v);
            }
            if v["type"] == "event" {
                self.event_buf.push(v);
            }
        }
    }

    /// Receive the next event (from buffer or WS).
    async fn recv_event(&mut self) -> Result<Value> {
        if !self.event_buf.is_empty() {
            return Ok(self.event_buf.remove(0));
        }
        loop {
            let msg = timeout(Duration::from_secs(30), self.read.next())
                .await
                .map_err(|_| anyhow::anyhow!("timeout waiting for event"))?
                .ok_or_else(|| anyhow::anyhow!("WS stream closed"))?
                .map_err(|e| anyhow::anyhow!("WS recv error: {e}"))?;
            let text = match msg {
                Message::Text(t) => t,
                _ => continue,
            };
            let v: Value = serde_json::from_str(&text)?;
            if v["type"] == "event" {
                return Ok(v);
            }
        }
    }

    /// Send a subscribe frame.
    async fn subscribe(&mut self, events: &[&str]) -> Result<()> {
        let frame = json!({ "type": "subscribe", "events": events });
        self.write
            .send(Message::Text(frame.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("WS send error: {e}"))?;
        Ok(())
    }

    /// Drain the initial session.ready event that the server pushes on every new connection.
    async fn drain_ready(&mut self) -> Result<()> {
        loop {
            // First check the buffer
            if let Some(idx) = self
                .event_buf
                .iter()
                .position(|e| e["event"] == "session.ready")
            {
                self.event_buf.remove(idx);
                return Ok(());
            }
            let msg = timeout(Duration::from_secs(10), self.read.next())
                .await
                .map_err(|_| anyhow::anyhow!("timeout waiting for session.ready"))?
                .ok_or_else(|| anyhow::anyhow!("WS closed before session.ready"))?
                .map_err(|e| anyhow::anyhow!("WS error: {e}"))?;
            let text = match msg {
                Message::Text(t) => t,
                _ => continue,
            };
            let v: Value = serde_json::from_str(&text)?;
            if v["type"] == "event" {
                if v["event"] == "session.ready" {
                    return Ok(());
                }
                self.event_buf.push(v);
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Basic ATP test: connect → session/ready → proc/list.
#[tokio::test]
async fn test_basic() -> Result<()> {
    let _span = span!(Level::INFO, "test_basic").entered();
    let (mut server, port, api_key) = spawn_debug_server().await?;
    let token = login_http(port, &api_key).await?;

    let mut ws = WsConn::connect(&token, port).await?;
    ws.drain_ready().await?;

    // session/ready → reply body should be "ack"
    let reply = ws.cmd("session", "ready", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false), "session/ready failed: {reply}");
    assert_eq!(reply["body"], "ack");

    // proc/list → reply body should be an array
    let reply = ws.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false), "proc/list failed: {reply}");
    assert!(reply["body"].is_array(), "expected array, got: {}", reply["body"]);

    server.kill().await?;
    info!("test_basic passed");
    Ok(())
}

/// Error handling: bad credential → 401; bad WS token → upgrade rejected.
#[tokio::test]
async fn test_errors() -> Result<()> {
    let _span = span!(Level::INFO, "test_errors").entered();
    let (mut server, port, _api_key) = spawn_debug_server().await?;

    // Bad credential → 401
    let client = reqwest::Client::new();
    let url = format!("http://localhost:{}/atp/auth/login", port);
    let resp = client
        .post(&url)
        .json(&json!({"identity": "test", "credential": "bad-key"}))
        .send()
        .await?;
    assert_eq!(resp.status(), 401);

    // Bad WS token → upgrade rejected (connect_async returns Err)
    let result = WsConn::connect("bad-token", port).await;
    assert!(result.is_err(), "expected WS upgrade to fail with bad token");

    server.kill().await?;
    info!("test_errors passed");
    Ok(())
}

/// Events test: proc/spawn emits proc.start event.
#[tokio::test]
async fn test_events() -> Result<()> {
    let _span = span!(Level::INFO, "test_events").entered();
    let (mut server, port, api_key) = spawn_debug_server().await?;
    let token = login_http(port, &api_key).await?;

    let mut ws = WsConn::connect(&token, port).await?;
    ws.drain_ready().await?;
    ws.subscribe(&["*"]).await?;

    // Spawn a proc
    let reply = ws
        .cmd("proc", "spawn", json!({"cmd": ["echo", "hello"], "name": "test-proc"}))
        .await?;
    assert!(reply["ok"].as_bool().unwrap_or(false), "proc/spawn failed: {reply}");
    assert!(reply["body"]["pid"].is_number(), "expected pid in reply body");

    // Receive proc.start event
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "proc.start", "expected proc.start, got: {event}");
    assert!(event["body"]["pid"].is_number());

    server.kill().await?;
    info!("test_events passed");
    Ok(())
}

/// Full error paths: bad token upgrade, invalid op, malformed frame.
#[tokio::test]
async fn test_full_errors() -> Result<()> {
    let _span = span!(Level::INFO, "test_full_errors").entered();

    // First server: test bad WS token
    let (mut server, port, _api_key) = spawn_debug_server().await?;
    let result = WsConn::connect("completely-invalid-token", port).await;
    assert!(result.is_err(), "expected WS upgrade rejection with bad token");
    server.kill().await?;

    // Second server: test invalid op and malformed frame
    let (mut server2, port2, api_key) = spawn_debug_server().await?;
    let token = login_http(port2, &api_key).await?;

    let mut ws = WsConn::connect(&token, port2).await?;
    ws.drain_ready().await?;

    // Unknown proc op → error reply
    let reply = ws.cmd("proc", "unknown_op_xyz", json!({})).await?;
    assert!(!reply["ok"].as_bool().unwrap_or(true), "expected error for unknown op");
    assert!(reply["error"].is_object());

    // Malformed frame (no type field) → server silently drops it; no reply comes back.
    // We can only verify the connection stays alive.
    use futures_util::SinkExt as _;
    ws.write
        .send(Message::Text(r#"{"id":1,"method":"proc.list"}"#.to_string()))
        .await?;

    // Connection should still be usable
    let reply = ws.cmd("session", "ready", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));

    server2.kill().await?;
    info!("test_full_errors passed");
    Ok(())
}

/// Proc lifecycle: spawn → list → output event → status → kill → exit event → list empty.
#[tokio::test]
async fn test_proc_lifecycle() -> Result<()> {
    let _span = span!(Level::INFO, "test_proc_lifecycle").entered();
    let (mut server, port, api_key) = spawn_debug_server().await?;
    let token = login_http(port, &api_key).await?;

    let mut ws = WsConn::connect(&token, port).await?;
    ws.drain_ready().await?;
    ws.subscribe(&["*"]).await?;

    // Spawn a proc
    let reply = ws
        .cmd("proc", "spawn", json!({"cmd": ["echo", "hi"], "name": "test-proc"}))
        .await?;
    assert!(reply["ok"].as_bool().unwrap_or(false), "proc/spawn failed: {reply}");
    let pid = reply["body"]["pid"].as_u64().expect("pid in body");

    // Receive proc.start event
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "proc.start");
    assert_eq!(event["body"]["pid"], pid);

    // proc/list includes the spawned proc
    let reply = ws.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    let list = reply["body"].as_array().expect("list array");
    assert!(
        list.iter().any(|p| p["pid"] == pid),
        "pid {pid} not in list: {list:?}"
    );

    // Receive proc.output event (echo sends output event)
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "proc.output");
    assert_eq!(event["body"]["pid"], pid);

    // proc/stat
    let reply = ws.cmd("proc", "stat", json!({"id": pid})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    assert_eq!(reply["body"]["status"], "running");

    // Kill the proc
    let reply = ws.cmd("proc", "kill", json!({"id": pid})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));

    // Receive proc.exit event
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "proc.exit");
    assert_eq!(event["body"]["pid"], pid);

    // proc/list is now empty
    let reply = ws.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    let list = reply["body"].as_array().expect("list array");
    assert!(list.is_empty(), "expected empty list after kill");

    server.kill().await?;
    info!("test_proc_lifecycle passed");
    Ok(())
}

/// Reconnect: spawn proc, disconnect, reconnect, verify proc persists in list.
#[tokio::test]
async fn test_basic_reconnect() -> Result<()> {
    let _span = span!(Level::INFO, "test_basic_reconnect").entered();
    let (mut server, port, api_key) = spawn_debug_server().await?;
    let token = login_http(port, &api_key).await?;

    // First connection: spawn a proc
    let pid = {
        let mut ws = WsConn::connect(&token, port).await?;
        ws.drain_ready().await?;
        let reply = ws
            .cmd("proc", "spawn", json!({"cmd": ["sleep", "10"], "name": "persistent-proc"}))
            .await?;
        assert!(reply["ok"].as_bool().unwrap_or(false));
        reply["body"]["pid"].as_u64().expect("pid")
        // ws dropped here → WS connection closed
    };

    // Second connection: verify proc still in list
    let mut ws2 = WsConn::connect(&token, port).await?;
    ws2.drain_ready().await?;

    let reply = ws2.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    let list = reply["body"].as_array().expect("list array");
    assert!(
        list.iter().any(|p| p["pid"] == pid),
        "pid {pid} not in list after reconnect: {list:?}"
    );

    // Kill the proc
    ws2.cmd("proc", "kill", json!({"id": pid})).await?;

    server.kill().await?;
    info!("test_basic_reconnect passed");
    Ok(())
}

/// Agent spawn lifecycle: spawn → list → kill → list empty.
/// Covers PROJECT-SPAWN-001 G8.
#[tokio::test]
async fn test_agent_spawn_lifecycle() -> Result<()> {
    let _span = span!(Level::INFO, "test_agent_spawn_lifecycle").entered();
    let (mut server, port, api_key) = spawn_debug_server().await?;
    let token = login_http(port, &api_key).await?;

    let mut ws = WsConn::connect(&token, port).await?;
    ws.drain_ready().await?;
    ws.subscribe(&["*"]).await?;

    // Spawn an agent
    let reply = ws
        .cmd(
            "proc",
            "spawn",
            json!({"agent": "test-agent", "goal": "Say hello world"}),
        )
        .await?;
    assert!(reply["ok"].as_bool().unwrap_or(false), "spawn failed: {reply}");
    let pid = reply["body"]["pid"].as_u64().expect("pid") as u32;

    // Receive agent.spawned event
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "agent.spawned", "unexpected event: {event}");
    assert_eq!(event["body"]["pid"], pid);

    // proc/list includes the agent
    let reply = ws.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    let list = reply["body"].as_array().expect("list array");
    assert!(list.iter().any(|a| a["pid"] == pid));
    let agent = list.iter().find(|a| a["pid"] == pid).unwrap();
    assert_eq!(agent["name"], "test-agent");
    assert_eq!(agent["goal"], "Say hello world");

    // Kill the agent
    let reply = ws.cmd("proc", "kill", json!({"id": pid})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));

    // Receive agent.exit event
    let event = ws.recv_event().await?;
    assert_eq!(event["event"], "agent.exit", "unexpected event: {event}");
    assert_eq!(event["body"]["pid"], pid);

    // proc/list is empty
    let reply = ws.cmd("proc", "list", json!({})).await?;
    assert!(reply["ok"].as_bool().unwrap_or(false));
    let list = reply["body"].as_array().expect("list array");
    assert!(list.is_empty(), "expected empty list after kill");

    server.kill().await?;
    info!("test_agent_spawn_lifecycle passed");
    Ok(())
}
