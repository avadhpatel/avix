# ATP Protocol Review - 2024-10-25

## Current Implementation State

### WebSocket Connection & ATP Initialization (TUI/GUI/avix-core)

1. **Login (HTTP POST /atp/auth/login)**:
   - Client (avix-client-core/src/atp/client.rs): POST to `{server_url}/atp/auth/login` with `identity`/`credential`.
   - Server (avix-core/src/gateway/server.rs:handle_login): AuthService.login → ATPToken.issue(claims) → returns `{token, expiresAt, sessionId}`.
   - TUI (avix-cli/src/tui/app.rs): On 'c' key, ServerHandle::ensure_running() probes /atp/health, spawns `avix start --root` if down, then AtpClient::connect().
   - GUI (avix-app/src-tauri/src/atp_client/): Similar types, uses client-core.

2. **WebSocket Upgrade (/atp)**:
   - Client: WS to `{server_url.replace(http,ws)}/atp` with manual headers incl. `Authorization: Bearer {token}` (tokio_tungstenite).
   - Server (server.rs:handle_ws_upgrade): Extracts/validates token → claims.session_id/role → upgrade → sends `session.ready` event immediately.
   - Client: Auto-sends `{type: \"subscribe\", events: [\"*\"]}`.

3. **Post-handshake**:
   - Client Dispatcher (avix-client-core/src/atp/dispatcher.rs): RPC via `call(cmd)` (oneshot replies, 30s timeout), events via broadcast channel.
   - Server: Reader loop handles cmd/subscribe, dispatches via HandlerCtx/IPC, replies via mpsc. Events filtered by role/owner/subscribe via EventBus.
   - Ping/pong keepalive (30s ping, 40s timeout).

### Supported ATP Features (avix-core)

* **Message Types**: cmd/subscribe (client→server), reply/event (server→client). Full frame parse/serialize.
* **Auth**: ATPToken (HMAC/JWT-like claims), login, expiry check, session_id match.
* **Subscriptions**: Per-conn EventFilter, \"*\" or list.
* **RPCs**: dispatch → IPC → reply. ReplayGuard per-conn.
* **Domains/Ops** (limited):
  | Domain | Ops |
  |--------|-----|
  | proc | spawn/kill/list/status
  | fs | read/write
  | sys | info/reboot
* **Events**: All 16 kinds defined (AtpEventKind), but emission kernel-dependent.
* **Errors**: Full AtpErrorCode enum.

## Gaps / Partial / Bugs

* **Missing Ops/Domains**: Only ~10 ops vs. spec's 50+ (no auth.refresh, signal.send, cron.*, users.*, crews.*, cap.*, pipe.*).
* **Partial**:
  - proc: No pause/resume/wait/setcap.
  - fs: No list/stat/watch.
  - sys: No status/reload/shutdown/install/uninstall.
  - Events: Types defined, but kernel may not emit all.
* **Client**:
  - Dispatcher tests ignored (needs mock WS).
  - GUI atp_client/client.rs: Stub types (AgentCommand/AtpEvent) — not using shared client-core?
* **Bugs/Risks**:
  - No reconnect logic in AtpClient (dev_plans/client-gap-C).
  - TUI auto-spawns server but no stop on exit.
  - No TLS (dev/prod gap).
  - Limited logging in some paths.
* **Observability**: Tracing good, but no structured ATP logs (id/session).

## E2E Validation Setup

Scriptable full-system run (daemon + gateway + clients) w/ max verbosity.

### 1. Start Server (Debug Mode)
```
#!/bin/bash
# e2e-atp-test.sh
export RUST_LOG=trace,avix_core=trace,avix_gateway=trace,avix_client_core=trace
rm -rf test-run && mkdir -p test-run/logs test-run/avix
cargo run --bin avix-cli -- start --root ./test-run/avix --log-level trace &
SERVER_PID=$!
sleep 10  # Wait boot
curl -v http://localhost:9142/atp/health  # Probe (ok)
echo $SERVER_PID > server.pid
```
* Runs kernel/services/gateway. Logs to `test-run/logs/*.log` + stdout.
* Default: ws://localhost:9142/atp (user port).
* Admin: ws://localhost:9143/atp.

### 2. Interact (ATP Msgs)
* **Login**:
  ```
  curl -X POST http://localhost:9142/atp/auth/login \\
    -H 'Content-Type: application/json' \\
    -d '{"identity":"admin","credential":"changeme"}'
  ```
* **WS Client** (e.g., wscat): `wscat -c ws://localhost:9142/atp -H 'Authorization: Bearer $TOKEN'`
  * Send: `{\"type\":\"subscribe\",\"events\":[\"*\"]}`
  * Cmd: `{\"type\":\"cmd\",\"id\":\"c1\",\"token\":\"$TOKEN\",\"domain\":\"proc\",\"op\":\"list\",\"body\":{}}`
* **TUI Client**: New term: `cargo run --bin avix-cli -- tui` ('c' connect, 'a' spawn test-agent).
* **Capture**: `tail -f test-run/logs/gateway.log | grep ATP` or `RUST_LOG=trace`.

### 3. Stop
```
kill $(cat server.pid)
```

### 4. Agent Automation
```
# In agent bash (background=true)
bash e2e-atp-test.sh
# Wait, login/curl cmds, tui subprocess, verify logs w/ grep.
process_logs $SERVER_PID | grep -E \"ATP|session|cmd|hil\"
process_stop $SERVER_PID
```

## Next Steps for Implementation Agents

1. **coding-agent**: Implement missing ops/domains (priority: signal.send SIGPIPE/HIL, full proc, fs.watch). Follow ATP spec (docs/architecture/04-atp.md).
2. **testing-agent**: ATP unit/E2E tests (mock WS/IPC), coverage for all domains/events/errors. Fix dispatcher tests.
3. **usability-agent**: Validate TUI/GUI reconnect, HIL flows, notifications persistence.
4. **architect-agent**: Update specs/gui-cli-via-atp.md w/ exact flows. New dev plan: ATP-v2-full-domains.md.

Open Risks: Reconnect logic missing → clients drop on net hiccup. TLS off → prod insecure.
