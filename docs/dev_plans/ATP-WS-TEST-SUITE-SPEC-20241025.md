# ATP WebSocket Integration Test Suite Spec

## Date: 2024-10-25
## Author: Grok CLI Agent
## References: docs/dev_plans/ATP-PROTOCOL-REVIEW-20241025.md

## Overview
Automated Rust integration tests for ATP protocol over WebSocket. Validates end-to-end:
* Server startup in debug mode.
* Client WS connect/handshake (login → Bearer → ready → subscribe).
* RPC calls (supported: proc.*, fs.*, sys.*).
* Event reception/subscribe.
* Error handling.
* Cleanup.

Runs locally/CI as `cargo test` before 'release' commits. Covers 100% supported features, notes gaps.

## Location
`tests/integration/atp_ws.rs` (workspace tests/ dir) or `crates/avix-tests-integration/tests/atp_ws.rs`.

## Dependencies [dev-dependencies]
```
tokio = { version = \"1\", features = [\"full\"] }
tokio-tungstenite = { version = \"0.24\", features = [\"native-tls\"] }
tungstenite = \"0.24\"
reqwest = { version = \"0.12\", features = [\"json\"] }
serde_json = \"1\"
portpicker = \"0.1\"  # Dynamic port alloc
tempfile = \"3\"
anyhow = \"1\"
tracing = \"0.1\"  # Match avix logging
```
Add to root Cargo.toml or new test crate.

## Test Structure
```rust
#[cfg(test)]
mod tests {
    use tokio::process::Command;
    use tokio_tungstenite::{tungstenite::Message, client};
    // ...

    async fn spawn_debug_server() -> (Child, u16) {
        let port = portpicker::pick_unused_port().unwrap();
        let root = tempfile::tempdir().unwrap().path().to_string_lossy();
        let mut child = Command::new(\"cargo\")
            .arg(\"run\").arg(\"-p\").arg(\"avix-server\")  // Or avix-cli server
            .arg(\"--\").arg(\"--root\").arg(&root)
            .arg(\"--port\").arg(port.to_string())
            .arg(\"--log-level\").arg(\"trace\")
            .env(\"RUST_LOG\", \"avix=trace\")
            .spawn()?;
        tokio::time::sleep(Duration::from_secs(5)).await;  // Startup
        assert!(wait_for_port(port).await);
        (child, port)
    }

    async fn login(base_url: &str) -> String {
        let client = reqwest::Client::new();
        let res = client.post(format!(\"{base_url}/login\"))
            .json(&json!({\"user\": \"test\", \"pass\": \"test\"}))
            .send().await?;
        res.json::<HashMap<String, String>>().await?[\"token\"].clone()
    }

    async fn atp_test_sequence(url: String, token: String) {
        let (ws_stream, _) = client(&format!(\"ws://{url}/ws\"), 
            tungstenite::client::IntoClientRequest::data(|_| {
                Request::builder()
                    .header(\"Authorization\", format!(\"Bearer {token}\"))
                    .uri(&format!(\"ws://{url}/ws\"))
                    .body(())
            })).await?;

        let (mut write, mut read) = ws_stream.split();

        // session.ready
        write.send(Message::Text(r#\"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.ready\"}\"#.into())).await?;
        let msg = read.next().await?; assert_eq!(msg? , expected_ready_ack);

        // subscribe
        // RPC proc.list → assert procs
        // proc.start \"echo hi\" → wait event proc_start/output/exit
        // Errors
        // unsubscribe
    }

    #[tokio::test]
    async fn test_atp_full_cycle() {
        let (server, port) = spawn_debug_server().await;
        let url = format!(\"localhost:{}\", port);
        let token = login(&url).await;
        atp_test_sequence(url, token).await;
        server.kill().await?;
    }

    #[tokio::test]
    async fn test_errors() { /* invalid token, bad method */ }
}
```

## Coverage Matrix
| Feature | Test Name | Status |
|---------|-----------|--------|
| Connect/Ready | test_connect_ready | Supported |
| Subscribe/Events | test_subscribe_proc | Supported |
| proc.list/start/stop | test_proc_lifecycle | Supported |
| fs.read/write | test_fs_ops | Supported |
| Errors | test_invalid_* | Supported |
| Reconnect | test_reconnect | Gap (todo) |

## Run & CI
* Local: `cargo test tests/integration/atp_ws -- --nocapture`
* CI/Release gate: `cargo make test.integration` or `cargo test --workspace --exclude avix-app`
* Logs: Captured via tracing subscriber, assert log contains ATP frames.

## Risks/Notes
* Server binary name: Confirm avix-server or avix-cli.
* Test data: Server --init-test-data or seed fs/procs in spawn.
* Gaps: Skip missing ops until impl.
* Perf: Add timeout 30s per test.

## Next
Hand to program-manager-agent for dev plan & agent coordination.