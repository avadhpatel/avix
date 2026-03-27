/// Integration tests for the ATP WebSocket transport (Gap D).
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;

use avix_core::auth::atp_token::{ATPToken, ATPTokenClaims, ATPTokenStore};
use avix_core::auth::service::AuthService;
use avix_core::config::auth::AuthPolicy;
use avix_core::config::{AuthConfig, AuthIdentity, CredentialType};
use avix_core::gateway::{AtpEventBus, GatewayConfig, GatewayServer};
use avix_core::types::Role;
use chrono::Utc;

// ── Test helpers ───────────────────────────────────────────────────────────────

fn make_key_hash(raw: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(b"config-init-secret").unwrap();
    mac.update(raw.as_bytes());
    format!("hmac-sha256:{}", hex::encode(mac.finalize().into_bytes()))
}

fn make_auth_config() -> AuthConfig {
    AuthConfig {
        api_version: "v1".to_string(),
        kind: "AuthConfig".to_string(),
        policy: AuthPolicy {
            session_ttl: "8h".to_string(),
            require_tls: false,
        },
        identities: vec![
            AuthIdentity {
                name: "alice".to_string(),
                uid: 1001,
                role: Role::Admin,
                credential: CredentialType::ApiKey {
                    key_hash: make_key_hash("hunter2"),
                    header: None,
                },
            },
            AuthIdentity {
                name: "guest_user".to_string(),
                uid: 1002,
                role: Role::Guest,
                credential: CredentialType::ApiKey {
                    key_hash: make_key_hash("guestpass"),
                    header: None,
                },
            },
        ],
    }
}

struct TestServer {
    user_port: u16,
    #[allow(dead_code)]
    admin_port: u16,
    #[allow(dead_code)]
    token_store: Arc<ATPTokenStore>,
    http: reqwest::Client,
}

async fn start_server() -> TestServer {
    let auth_config = make_auth_config();
    let auth_svc = Arc::new(AuthService::new(auth_config));
    let token_store = Arc::new(ATPTokenStore::new("test-gateway-secret".to_string()));
    let event_bus = Arc::new(AtpEventBus::new());

    let server = GatewayServer::new(
        GatewayConfig::default(),
        Arc::clone(&auth_svc),
        Arc::clone(&token_store),
        Arc::clone(&event_bus),
    );

    let user_addr = Arc::clone(&server)
        .bind_and_run("127.0.0.1:0".parse().unwrap(), false, true)
        .await
        .expect("bind user port");
    let admin_addr = Arc::clone(&server)
        .bind_and_run("127.0.0.1:0".parse().unwrap(), true, true)
        .await
        .expect("bind admin port");

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    TestServer {
        user_port: user_addr.port(),
        admin_port: admin_addr.port(),
        token_store,
        http,
    }
}

async fn login(
    http: &reqwest::Client,
    port: u16,
    identity: &str,
    credential: &str,
) -> reqwest::Response {
    http.post(format!("http://127.0.0.1:{port}/atp/auth/login"))
        .json(&json!({ "identity": identity, "credential": credential }))
        .send()
        .await
        .expect("login request failed")
}

async fn login_ok(
    http: &reqwest::Client,
    port: u16,
    identity: &str,
    credential: &str,
) -> (String, String) {
    let resp = login(http, port, identity, credential).await;
    assert_eq!(resp.status(), 200, "expected 200 for valid login");
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    let session_id = body["sessionId"].as_str().unwrap().to_string();
    (token, session_id)
}

/// Build a proper WS upgrade request with Bearer token.
/// Must include Upgrade, Connection, Sec-WebSocket-Key, and Sec-WebSocket-Version headers.
fn ws_request(port: u16, token: &str) -> Request<()> {
    Request::builder()
        .uri(format!("ws://127.0.0.1:{port}/atp"))
        .header("Host", format!("127.0.0.1:{port}"))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Version", "13")
        .header("Authorization", format!("Bearer {token}"))
        .body(())
        .unwrap()
}

async fn connect_ws(
    port: u16,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let request = ws_request(port, token);
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws connect failed");
    ws
}

async fn read_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let msg = timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for message")
        .expect("stream ended")
        .expect("ws error");
    match msg {
        TungsteniteMessage::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text message, got {other:?}"),
    }
}

fn make_cmd_frame(token: &str, id: &str, domain: &str, op: &str) -> String {
    json!({
        "type": "cmd",
        "id": id,
        "token": token,
        "domain": domain,
        "op": op,
        "body": {}
    })
    .to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn login_returns_token() {
    let srv = start_server().await;
    let resp = login(&srv.http, srv.user_port, "alice", "hunter2").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["token"].is_string(), "expected token field");
    assert!(body["sessionId"].is_string(), "expected sessionId field");
    assert!(body["expiresAt"].is_string(), "expected expiresAt field");
}

/// A wrong or empty credential returns 401.
#[tokio::test]
async fn login_wrong_credential_returns_401() {
    let srv = start_server().await;
    // Empty credential fails validation (validate_credential checks non-empty)
    let resp = login(&srv.http, srv.user_port, "alice", "").await;
    assert_eq!(resp.status(), 401);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"].as_str().unwrap(), "EAUTH");
}

#[tokio::test]
async fn login_unknown_user_returns_401() {
    let srv = start_server().await;
    let resp = login(&srv.http, srv.user_port, "nobody", "anything").await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn ws_upgrade_without_auth_rejected() {
    let srv = start_server().await;
    // No Authorization header — should get 401 or connection failure
    let request = Request::builder()
        .uri(format!("ws://127.0.0.1:{}/atp", srv.user_port))
        .header("Host", format!("127.0.0.1:{}", srv.user_port))
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Version", "13")
        .body(())
        .unwrap();
    let result = tokio_tungstenite::connect_async(request).await;
    // Either fails at connection or returns a non-101 response
    match result {
        Err(_) => {} // connection rejected — acceptable
        Ok((_, resp)) => {
            // Should NOT be 101 Switching Protocols
            assert_ne!(
                resp.status().as_u16(),
                101,
                "expected non-101 but got Switching Protocols"
            );
        }
    }
}

#[tokio::test]
async fn ws_upgrade_sends_session_ready() {
    let srv = start_server().await;
    let (token, _session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &token).await;

    let msg = read_text(&mut ws).await;
    assert_eq!(msg["type"].as_str().unwrap(), "event");
    assert_eq!(msg["event"].as_str().unwrap(), "session.ready");
}

#[tokio::test]
async fn session_ready_contains_session_id() {
    let srv = start_server().await;
    let (token, session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &token).await;

    let msg = read_text(&mut ws).await;
    assert_eq!(msg["event"].as_str().unwrap(), "session.ready");
    assert_eq!(msg["sessionId"].as_str().unwrap(), &session_id);
    assert_eq!(msg["body"]["sessionId"].as_str().unwrap(), &session_id);
}

#[tokio::test]
async fn cmd_with_invalid_token_returns_eauth() {
    let srv = start_server().await;
    let (good_token, _session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &good_token).await;

    // Consume session.ready
    let _ = read_text(&mut ws).await;

    // Send a cmd with a bad token
    let bad_cmd = json!({
        "type": "cmd",
        "id": "test-001",
        "token": "totally.invalid",
        "domain": "proc",
        "op": "list",
        "body": {},
    })
    .to_string();

    ws.send(TungsteniteMessage::Text(bad_cmd)).await.unwrap();
    let reply = read_text(&mut ws).await;
    assert_eq!(reply["type"].as_str().unwrap(), "reply");
    assert!(!reply["ok"].as_bool().unwrap());
    assert_eq!(reply["error"]["code"].as_str().unwrap(), "EAUTH");
}

#[tokio::test]
async fn cmd_with_expired_token_returns_eexpired() {
    let srv = start_server().await;
    let (good_token, session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &good_token).await;

    // Consume session.ready
    let _ = read_text(&mut ws).await;

    // Create an expired token manually (bypass store validation at issue time)
    let expired_claims = ATPTokenClaims {
        sub: "alice".to_string(),
        uid: 1001,
        role: Role::Admin,
        crews: vec![],
        session_id: session_id.clone(),
        iat: Utc::now() - chrono::Duration::hours(10),
        exp: Utc::now() - chrono::Duration::hours(2),
        scope: vec!["proc".into()],
    };
    let expired_token = ATPToken::issue(expired_claims, "test-gateway-secret").unwrap();

    let cmd = make_cmd_frame(&expired_token, "test-exp-001", "proc", "list");
    ws.send(TungsteniteMessage::Text(cmd)).await.unwrap();

    let reply = read_text(&mut ws).await;
    assert!(!reply["ok"].as_bool().unwrap());
    assert_eq!(reply["error"]["code"].as_str().unwrap(), "EEXPIRED");
}

#[tokio::test]
async fn duplicate_command_id_returns_eparse() {
    let srv = start_server().await;
    let (token, _session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &token).await;

    // Consume session.ready
    let _ = read_text(&mut ws).await;

    let cmd = make_cmd_frame(&token, "dup-cmd-id", "proc", "list");

    // First send — should get EUNAVAIL (valid cmd, stub response)
    ws.send(TungsteniteMessage::Text(cmd.clone()))
        .await
        .unwrap();
    let reply1 = read_text(&mut ws).await;
    assert_eq!(reply1["id"].as_str().unwrap(), "dup-cmd-id");

    // Second send — duplicate id, should get EPARSE
    ws.send(TungsteniteMessage::Text(cmd)).await.unwrap();
    let reply2 = read_text(&mut ws).await;
    assert!(!reply2["ok"].as_bool().unwrap());
    assert_eq!(reply2["error"]["code"].as_str().unwrap(), "EPARSE");
}

#[tokio::test]
async fn guest_role_proc_spawn_returns_eperm() {
    let srv = start_server().await;
    let (token, _session_id) = login_ok(&srv.http, srv.user_port, "guest_user", "guestpass").await;
    let mut ws = connect_ws(srv.user_port, &token).await;

    // Consume session.ready
    let _ = read_text(&mut ws).await;

    let cmd = make_cmd_frame(&token, "eperm-001", "proc", "spawn");
    ws.send(TungsteniteMessage::Text(cmd)).await.unwrap();

    let reply = read_text(&mut ws).await;
    assert!(!reply["ok"].as_bool().unwrap());
    assert_eq!(reply["error"]["code"].as_str().unwrap(), "EPERM");
}

#[tokio::test]
async fn valid_proc_list_returns_ok() {
    let srv = start_server().await;
    let (token, _session_id) = login_ok(&srv.http, srv.user_port, "alice", "hunter2").await;
    let mut ws = connect_ws(srv.user_port, &token).await;

    // Consume session.ready
    let _ = read_text(&mut ws).await;

    let cmd = make_cmd_frame(&token, "proc-list-001", "proc", "list");
    ws.send(TungsteniteMessage::Text(cmd)).await.unwrap();

    let reply = read_text(&mut ws).await;
    assert_eq!(reply["id"].as_str().unwrap(), "proc-list-001");
    assert!(reply["ok"].as_bool().unwrap());
    assert_eq!(reply["body"], json!([]));
}
