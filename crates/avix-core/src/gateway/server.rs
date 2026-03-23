use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::stream::{SplitSink, StreamExt};
use futures::SinkExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::auth::atp_token::{ATPTokenClaims, ATPTokenStore};
use crate::auth::service::AuthService;
use crate::gateway::atp::frame::{AtpEvent, AtpFrame, AtpReply};
use crate::gateway::atp::types::AtpEventKind;
use crate::gateway::config::GatewayConfig;
use crate::gateway::event_bus::AtpEventBus;
use crate::gateway::handlers::{dispatch, HandlerCtx, LiveIpcRouter, NullIpcRouter};
use crate::gateway::replay::ReplayGuard;
use crate::gateway::validator::validate_cmd;
use crate::ipc::IpcClient;

#[derive(Clone)]
struct AppState {
    auth_svc: Arc<AuthService>,
    token_store: Arc<ATPTokenStore>,
    // Will be used in Gap F for event broadcasting
    #[allow(dead_code)]
    event_bus: Arc<AtpEventBus>,
    is_admin_port: bool,
    handler_ctx: Arc<HandlerCtx>,
}

pub struct GatewayServer {
    config: GatewayConfig,
    auth_svc: Arc<AuthService>,
    token_store: Arc<ATPTokenStore>,
    event_bus: Arc<AtpEventBus>,
}

impl GatewayServer {
    pub fn new(
        config: GatewayConfig,
        auth_svc: Arc<AuthService>,
        token_store: Arc<ATPTokenStore>,
        event_bus: Arc<AtpEventBus>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            auth_svc,
            token_store,
            event_bus,
        })
    }

    /// Bind both ports and run until either exits. Returns the bound addresses.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<(SocketAddr, SocketAddr)> {
        let user_addr = self.config.user_addr;
        let admin_addr = self.config.admin_addr;

        let user_bound = Arc::clone(&self).bind_and_run(user_addr, false).await?;
        let admin_bound = Arc::clone(&self).bind_and_run(admin_addr, true).await?;

        Ok((user_bound, admin_bound))
    }

    /// Bind to a specific address and return the bound addr (for test: use port 0).
    pub async fn bind_and_run(
        self: Arc<Self>,
        addr: SocketAddr,
        is_admin_port: bool,
    ) -> anyhow::Result<SocketAddr> {
        // Build IPC router: use configured socket, fall back to env var, then null.
        let ipc: Arc<dyn crate::gateway::handlers::IpcRouter> = self
            .config
            .kernel_sock
            .clone()
            .or_else(|| std::env::var("AVIX_KERNEL_SOCK").ok().map(Into::into))
            .map(|path| -> Arc<dyn crate::gateway::handlers::IpcRouter> {
                Arc::new(LiveIpcRouter::new(IpcClient::new(path)))
            })
            .unwrap_or_else(|| Arc::new(NullIpcRouter));

        let handler_ctx = Arc::new(HandlerCtx {
            ipc,
            token_store: Arc::clone(&self.token_store),
            auth_svc: Arc::clone(&self.auth_svc),
        });

        let state = AppState {
            auth_svc: Arc::clone(&self.auth_svc),
            token_store: Arc::clone(&self.token_store),
            event_bus: Arc::clone(&self.event_bus),
            is_admin_port,
            handler_ctx,
        };

        let app = Router::new()
            .route("/atp/auth/login", post(handle_login))
            .route("/atp", get(handle_ws_upgrade))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound_addr = listener.local_addr()?;

        info!(addr = %bound_addr, is_admin_port, "gateway listener bound");

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                warn!(error = %e, "gateway server error");
            }
        });

        Ok(bound_addr)
    }
}

// ── Login endpoint ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginRequest {
    identity: String,
    credential: String,
}

async fn handle_login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    match state.auth_svc.login(&body.identity, &body.credential).await {
        Ok(session) => {
            let now = Utc::now();
            let exp = now + chrono::Duration::hours(8);
            let claims = ATPTokenClaims {
                sub: session.identity_name.clone(),
                uid: session.uid,
                role: session.role,
                crews: session.crews.clone(),
                session_id: session.session_id.clone(),
                iat: now,
                exp,
                scope: session.scope.clone(),
            };
            match state.token_store.issue(claims).await {
                Ok(token) => Json(json!({
                    "token": token,
                    "expiresAt": exp.to_rfc3339(),
                    "sessionId": session.session_id,
                }))
                .into_response(),
                Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "EAUTH", "message": "invalid credential"})),
        )
            .into_response(),
    }
}

// ── WebSocket upgrade ──────────────────────────────────────────────────────────

async fn handle_ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    // Extract Bearer token from Authorization header
    let token_str = match headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        Some(t) => t.to_string(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "EAUTH", "message": "missing Authorization header"})),
            )
                .into_response();
        }
    };

    // Validate the token before upgrading
    let claims = match state.token_store.validate(&token_str).await {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "EAUTH", "message": "invalid or expired token"})),
            )
                .into_response();
        }
    };

    let session_id = claims.session_id.clone();

    ws.on_upgrade(move |socket| run_connection(socket, state, session_id, token_str))
}

// ── Connection loop ────────────────────────────────────────────────────────────

async fn run_connection(ws: WebSocket, state: AppState, session_id: String, _token_str: String) {
    let (ws_sender, mut ws_receiver) = ws.split();
    let (tx, rx) = mpsc::channel::<WsOutMsg>(64);

    // Push session.ready
    let ready = AtpEvent::new(
        AtpEventKind::SessionReady,
        &session_id,
        json!({ "sessionId": session_id }),
    );
    if let Ok(text) = serde_json::to_string(&ready) {
        let _ = tx.send(WsOutMsg::Text(text)).await;
    }

    let replay_guard = ReplayGuard::new();

    // Writer task
    let writer = tokio::spawn(writer_task(ws_sender, rx));

    // Keep-alive interval (30s ping, 10s pong timeout)
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.tick().await; // consume the immediate first tick
    let ping_tx = tx.clone();

    let mut last_pong = tokio::time::Instant::now();

    // Reader loop
    loop {
        tokio::select! {
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_text_frame(&text, &state, &replay_guard, &session_id, &tx).await;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = tokio::time::Instant::now();
                        debug!(session_id = %session_id, "pong received");
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        debug!(session_id = %session_id, "ws connection closed");
                        break;
                    }
                    _ => {}
                }
            }
            _ = ping_interval.tick() => {
                // Check if last pong was more than 40s ago (30s interval + 10s grace)
                if last_pong.elapsed() > Duration::from_secs(40) {
                    warn!(session_id = %session_id, "ping timeout, closing connection");
                    break;
                }
                last_pong = tokio::time::Instant::now();
                let _ = ping_tx.send(WsOutMsg::Ping).await;
            }
        }
    }

    writer.abort();
}

async fn handle_text_frame(
    text: &str,
    state: &AppState,
    replay_guard: &ReplayGuard,
    session_id: &str,
    tx: &mpsc::Sender<WsOutMsg>,
) {
    match AtpFrame::parse(text) {
        Ok(AtpFrame::Cmd(cmd)) => {
            let cmd_id = cmd.id.clone();
            let reply = match validate_cmd(
                cmd,
                &state.token_store,
                replay_guard,
                session_id,
                state.is_admin_port,
            )
            .await
            {
                Ok(validated) => {
                    // Check token expiry — send event if expiring soon
                    if state
                        .token_store
                        .is_expiring_soon(&validated.cmd.token)
                        .await
                        .unwrap_or(false)
                    {
                        let ev = AtpEvent::new(AtpEventKind::TokenExpiring, session_id, json!({}));
                        if let Ok(s) = serde_json::to_string(&ev) {
                            let _ = tx.send(WsOutMsg::Text(s)).await;
                        }
                    }
                    dispatch(validated, &state.handler_ctx).await
                }
                Err(e) => AtpReply::err(cmd_id, e),
            };

            if let Ok(s) = serde_json::to_string(&reply) {
                let _ = tx.send(WsOutMsg::Text(s)).await;
            }
        }
        Ok(AtpFrame::Subscribe(_)) => {
            // Handled in Gap F — ignore for now
        }
        Err(_) => {
            // Ignore malformed frames silently
        }
    }
}

// ── Writer task ────────────────────────────────────────────────────────────────

enum WsOutMsg {
    Text(String),
    Ping,
}

async fn writer_task(mut sender: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<WsOutMsg>) {
    while let Some(msg) = rx.recv().await {
        let ws_msg = match msg {
            WsOutMsg::Text(t) => Message::Text(t),
            WsOutMsg::Ping => Message::Ping(vec![]),
        };
        if sender.send(ws_msg).await.is_err() {
            break;
        }
    }
}
