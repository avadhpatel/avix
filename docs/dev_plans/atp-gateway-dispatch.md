# ATP Gateway Dispatch Gap: Full WS /atp Handler

## Spec Reference
CLAUDE.md ADR-05 (fresh IPC/call), docs/architecture/12-avix-clients s3 atp::dispatcher, s6 Flows (ATP Cmd → IPC → Reply/Event).

## Goals
* Complete daemon phase4_atp_gateway: axum WS /atp auth ATPToken → IPC router.svc dispatch → tool/proc exec → Reply frame stream.
* Bidirectional: Event/HIL → WS broadcast subscribers.
* Health /atp/health, subscribe * /agent.*.
* client-core dispatcher routes domain/op (proc.spawn → kernel.proc.spawn IPC).

## Dependencies
* axum ws feature, tokio-tungstenite? IPC client from avix-core.

## Files to Create/Edit
* crates/avix-core/src/gateway/atp_handler.rs (ws extract token → IPC).
* crates/avix-core/src/bootstrap/mod.rs phase4_atp_gateway impl.
* crates/avix-core/Cargo.toml axum = {version = \"0.7\", features = [\"ws\"] }.
* crates/avix-core/src/ipc/client.rs (router.svc call).
* tests/gateway_dispatch.rs (mock IPC → reply).

## Detailed Tasks
1. Cargo deps axum/tower-http/ws.
2. bootstrap phase4:
```
rust
async fn phase4_atp_gateway(&mut self, port: u16) {
  let app = axum::Router::new()
    .route(\"/atp/health\", get(health))
    .route(\"/atp\", get(ws_handler));
  let listener = TcpListener::bind(format!(\"0.0.0.0:{}\", port)).await?;
  axum::serve(listener, app).await?;
}
```
3. ws_handler:
```
rust
async fn ws_handler(ws: WsUpgrade, headers: HeaderMap, state: State<AppState>) -> Response {
  let token = headers.get(\"authorization\").and_then(|h| h.to_str().ok())?.replace(\"Bearer \", \"\");
  let claims = validate_token(&token)?;  // session_id
  ws.on_upgrade(move |socket| handle_ws(socket, claims.session_id, state))
}
```
4. handle_ws:
```
rust
async fn handle_ws(mut socket: WebSocket, session_id: String, state: State<AppState>) {
  while let Some(msg) = socket.recv().await {
    let frame: Frame = serde_json::from_str(&msg.text()?)?;
    if frame.frame_type == \"subscribe\" {
      // add subscriber session_id
    } else if let Frame::Cmd(cmd) = frame {
      let reply = state.dispatcher.call(&cmd, session_id).await?;  // IPC router → tool
      socket.send(Message::Text(serde_json::to_string(&reply)?)).await?;
    }
  }
}
```
5. dispatcher.call(cmd, session): IPC router.svc domain/op params → exec → Reply.
6. Event broadcast: state.event_emitter.on_event → all subscribers WS send.

7. Tests: mock IPC → reply, WS frame parse/dispatch.

## Verify
* avix start → curl ws://localhost:9142/atp subscribe → ok.
* avix agent spawn → Cmd IPC → PID Reply stream.
* Events → WS push subscribers.

Est: 6h