use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::AvixError;
use crate::ipc::client::IpcClient;
use crate::ipc::message::{IpcMessage, JsonRpcRequest, JsonRpcResponse};
use crate::ipc::server::{IpcServer, IpcServerHandle};
use crate::mcp_bridge::client::McpToolInfo;
use crate::mcp_bridge::config::McpConfig;
use crate::mcp_bridge::connection::McpServerConnection;
use crate::mcp_bridge::meta_tools;

/// Shared state across the IPC handler and the health monitor.
type Connections = Arc<RwLock<HashMap<String, McpServerConnection>>>;

// ── Tool registration helpers ─────────────────────────────────────────────────

/// Build and send a single `ipc.tool-add` request for the given tool list.
async fn register_tools_with_avix(
    kernel_sock: &Path,
    token: &str,
    tools: &[McpToolInfo],
    namespace: &str,
) -> Result<Vec<String>, AvixError> {
    if tools.is_empty() {
        return Ok(Vec::new());
    }

    let tool_specs: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            let avix_name = format!("{}{}", namespace, t.name);
            serde_json::json!({
                "name": avix_name,
                "descriptor": {
                    "description": t.description,
                    "streaming": false,
                    "input_schema": t.input_schema
                },
                "visibility": "all"
            })
        })
        .collect();

    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Uuid::new_v4().to_string(),
        method: "ipc.tool-add".into(),
        params: serde_json::json!({
            "_token": token,
            "tools": tool_specs
        }),
    };

    let client = IpcClient::new(kernel_sock.to_path_buf());
    let resp = client.call(req).await?;
    if let Some(err) = &resp.error {
        return Err(AvixError::McpProtocol(format!(
            "ipc.tool-add failed: {} (code {})",
            err.message, err.code
        )));
    }

    let names: Vec<String> = tools
        .iter()
        .map(|t| format!("{}{}", namespace, t.name))
        .collect();
    Ok(names)
}

/// Send `ipc.tool-remove` for the given tool names.
async fn deregister_tools_from_avix(
    kernel_sock: &Path,
    token: &str,
    tool_names: &[String],
    drain: bool,
) -> Result<(), AvixError> {
    if tool_names.is_empty() {
        return Ok(());
    }

    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Uuid::new_v4().to_string(),
        method: "ipc.tool-remove".into(),
        params: serde_json::json!({
            "_token": token,
            "tools": tool_names,
            "reason": "mcp-bridge shutting down",
            "drain": drain
        }),
    };

    let client = IpcClient::new(kernel_sock.to_path_buf());
    let resp = client.call(req).await?;
    if let Some(err) = &resp.error {
        warn!(
            error = %err.message,
            "ipc.tool-remove returned error — continuing shutdown"
        );
    }
    Ok(())
}

// ── Health monitor ────────────────────────────────────────────────────────────

async fn health_monitor(
    connections: Connections,
    kernel_sock: PathBuf,
    service_token: String,
    interval: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // consume the immediate first tick

    loop {
        ticker.tick().await;

        // Collect server names to check (read lock).
        let server_names: Vec<String> = {
            let conns = connections.read().await;
            conns.keys().cloned().collect()
        };

        for server_name in server_names {
            let was_healthy = {
                let conns = connections.read().await;
                conns.get(&server_name).is_some_and(|c| c.is_healthy())
            };

            let was_degraded = {
                let conns = connections.read().await;
                conns.get(&server_name).is_some_and(|c| c.is_degraded())
            };

            // Attempt to call tools/list as a health check.
            let check_result = {
                let mut conns = connections.write().await;
                if let Some(conn) = conns.get_mut(&server_name) {
                    conn.discover_tools().await.map(|_| ())
                } else {
                    Ok(())
                }
            };

            match check_result {
                Ok(()) if was_degraded => {
                    // Server recovered — re-register its tools.
                    info!(server = %server_name, "MCP server recovered — re-registering tools");
                    let (namespace, tools) = {
                        let conns = connections.read().await;
                        if let Some(conn) = conns.get(&server_name) {
                            (conn.namespace().to_string(), conn.tools().to_vec())
                        } else {
                            continue;
                        }
                    };
                    match register_tools_with_avix(&kernel_sock, &service_token, &tools, &namespace)
                        .await
                    {
                        Ok(names) => {
                            info!(server = %server_name, count = names.len(), "re-registered tools after recovery");
                        }
                        Err(e) => {
                            warn!(server = %server_name, error = %e, "failed to re-register tools after recovery");
                        }
                    }
                }
                Err(e) if was_healthy => {
                    // Server just went down — remove its tools.
                    warn!(server = %server_name, error = %e, "MCP server degraded");
                    let tool_names: Vec<String> = {
                        let conns = connections.read().await;
                        conns
                            .get(&server_name)
                            .map(|c| c.tool_names().collect())
                            .unwrap_or_default()
                    };
                    if let Err(e) =
                        deregister_tools_from_avix(&kernel_sock, &service_token, &tool_names, false)
                            .await
                    {
                        warn!(server = %server_name, error = %e, "failed to deregister degraded tools");
                    }
                }
                _ => {}
            }
        }
    }
}

// ── McpBridgeRunner ───────────────────────────────────────────────────────────

/// Orchestrates the full MCP bridge lifecycle.
pub struct McpBridgeRunner {
    config: McpConfig,
    kernel_sock: PathBuf,
    service_token: String,
    svc_sock: PathBuf,
}

impl McpBridgeRunner {
    pub fn new(
        config: McpConfig,
        kernel_sock: PathBuf,
        service_token: String,
        svc_sock: PathBuf,
    ) -> Self {
        Self {
            config,
            kernel_sock,
            service_token,
            svc_sock,
        }
    }

    /// Start the bridge:
    /// 1. Connect to each MCP server
    /// 2. Discover tools
    /// 3. Register tools via `ipc.tool-add`
    /// 4. Bind `IpcServer` on `svc_sock`
    /// 5. Start health monitor
    pub async fn start(self) -> Result<RunningBridge, AvixError> {
        let mut connections: HashMap<String, McpServerConnection> = HashMap::new();
        let mut registered_tool_names: Vec<String> = Vec::new();

        for (server_name, server_config) in &self.config.mcp_servers {
            let namespace = server_config.tool_namespace(server_name);
            let health_interval_secs = server_config.health_check_interval_secs;

            match McpServerConnection::connect(server_name, server_config.clone()).await {
                Ok(mut conn) => {
                    match conn.discover_tools().await {
                        Ok(tools) => {
                            let tools_vec = tools.to_vec();
                            match register_tools_with_avix(
                                &self.kernel_sock,
                                &self.service_token,
                                &tools_vec,
                                &namespace,
                            )
                            .await
                            {
                                Ok(names) => {
                                    info!(
                                        server = %server_name,
                                        count = names.len(),
                                        "registered MCP tools"
                                    );
                                    registered_tool_names.extend(names);
                                }
                                Err(e) => {
                                    warn!(server = %server_name, error = %e, "failed to register tools — server will be skipped");
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            warn!(server = %server_name, error = %e, "failed to discover tools — server degraded");
                        }
                    }
                    connections.insert(server_name.clone(), conn);
                }
                Err(e) => {
                    warn!(server = %server_name, error = %e, "failed to connect to MCP server — skipping");
                }
            }
            let _ = health_interval_secs; // will be used by monitor
        }

        // Register bridge meta-tools (mcp/servers, mcp/server-status, mcp/tools).
        let meta_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Uuid::new_v4().to_string(),
            method: "ipc.tool-add".into(),
            params: serde_json::json!({
                "_token": &self.service_token,
                "tools": meta_tools::meta_tool_descriptors()
            }),
        };
        let meta_client = IpcClient::new(self.kernel_sock.clone());
        match meta_client.call(meta_req).await {
            Ok(resp) if resp.error.is_none() => {
                info!("registered MCP bridge meta-tools");
                for name in meta_tools::ALL_META_TOOLS {
                    registered_tool_names.push(name.to_string());
                }
            }
            Ok(resp) => {
                warn!(
                    error = ?resp.error,
                    "failed to register MCP bridge meta-tools"
                );
            }
            Err(e) => {
                warn!(error = %e, "failed to register MCP bridge meta-tools");
            }
        }

        let connections = Arc::new(RwLock::new(connections));

        // Bind IPC server for handling tool calls.
        let (server, server_handle) = IpcServer::bind(self.svc_sock.clone())
            .await
            .map_err(|e| AvixError::Io(format!("failed to bind mcp-bridge socket: {e}")))?;

        let conns_for_handler = Arc::clone(&connections);
        let server_handle_clone = server_handle.clone();
        let server_join = tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let conns = Arc::clone(&conns_for_handler);
                    async move { handle_tool_call(msg, conns).await }
                })
                .await
            {
                debug!(error = %e, "mcp-bridge IpcServer stopped");
            }
        });

        // Determine health monitor interval from config (use minimum across servers,
        // default 30s if no servers).
        let monitor_interval_secs = self
            .config
            .mcp_servers
            .values()
            .map(|s| s.health_check_interval_secs)
            .min()
            .unwrap_or(30);

        let conns_for_health = Arc::clone(&connections);
        let kernel_sock_health = self.kernel_sock.clone();
        let token_health = self.service_token.clone();
        let health_join = tokio::spawn(async move {
            health_monitor(
                conns_for_health,
                kernel_sock_health,
                token_health,
                Duration::from_secs(monitor_interval_secs),
            )
            .await
        });

        Ok(RunningBridge {
            server_handle: server_handle_clone,
            server_join,
            health_join,
            service_token: self.service_token,
            kernel_sock: self.kernel_sock,
            registered_tool_names,
        })
    }
}

// ── Tool call handler ─────────────────────────────────────────────────────────

async fn handle_tool_call(msg: IpcMessage, connections: Connections) -> Option<JsonRpcResponse> {
    let req = match msg {
        IpcMessage::Request(r) => r,
        IpcMessage::Notification(_) => return None,
    };

    // Try meta-tools first (mcp/servers, mcp/server-status, mcp/tools).
    if let Some(resp) = meta_tools::handle_meta_tool(&req, &connections).await {
        return Some(resp);
    }

    let tool_name = &req.method;
    let params = req.params.clone();

    // Find the connection that owns this tool by namespace prefix.
    let server_name = {
        let conns = connections.read().await;
        conns
            .iter()
            .find(|(_, conn)| tool_name.starts_with(conn.namespace()))
            .map(|(name, _)| name.clone())
    };

    let server_name = match server_name {
        Some(n) => n,
        None => {
            return Some(JsonRpcResponse::err(
                &req.id,
                -32601,
                &format!("tool '{}' not found in any MCP server", tool_name),
                None,
            ));
        }
    };

    let result = {
        let mut conns = connections.write().await;
        if let Some(conn) = conns.get_mut(&server_name) {
            conn.forward_call(tool_name, params).await
        } else {
            return Some(JsonRpcResponse::err(
                &req.id,
                -32005,
                &format!("MCP server '{}' is unavailable", server_name),
                None,
            ));
        }
    };

    match result {
        Ok(value) => Some(JsonRpcResponse::ok(&req.id, value)),
        Err(e) => Some(JsonRpcResponse::err(
            &req.id,
            -32005,
            &format!("MCP call failed: {e}"),
            None,
        )),
    }
}

// ── RunningBridge ─────────────────────────────────────────────────────────────

/// A running MCP bridge instance. Call `shutdown()` to stop gracefully.
pub struct RunningBridge {
    server_handle: IpcServerHandle,
    server_join: tokio::task::JoinHandle<()>,
    health_join: tokio::task::JoinHandle<()>,
    service_token: String,
    kernel_sock: PathBuf,
    registered_tool_names: Vec<String>,
}

impl RunningBridge {
    pub fn registered_tool_names(&self) -> &[String] {
        &self.registered_tool_names
    }

    /// Graceful shutdown:
    /// 1. Cancel the IpcServer
    /// 2. Send `ipc.tool-remove` for all registered tools
    /// 3. Abort the health monitor
    pub async fn shutdown(self) -> Result<(), AvixError> {
        // 1. Stop accepting new connections.
        self.server_handle.cancel();
        let _ = self.server_join.await;

        // 2. Deregister all tools.
        if let Err(e) = deregister_tools_from_avix(
            &self.kernel_sock,
            &self.service_token,
            &self.registered_tool_names,
            false,
        )
        .await
        {
            warn!(error = %e, "failed to deregister MCP tools during shutdown");
        }

        // 3. Stop health monitor.
        self.health_join.abort();

        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tool(name: &str) -> McpToolInfo {
        McpToolInfo {
            name: name.into(),
            description: format!("Description for {name}"),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    #[tokio::test]
    async fn register_tools_builds_correct_wire_payload() {
        // Spin up a mock Unix socket that captures the request.
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("kernel.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        let capture = Arc::new(tokio::sync::Mutex::new(None::<serde_json::Value>));
        let capture_clone = Arc::clone(&capture);

        tokio::spawn(async move {
            if let Ok((mut conn, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut len_buf = [0u8; 4];
                if conn.read_exact(&mut len_buf).await.is_ok() {
                    let len = u32::from_le_bytes(len_buf) as usize;
                    let mut body = vec![0u8; len];
                    if conn.read_exact(&mut body).await.is_ok() {
                        let val: serde_json::Value = serde_json::from_slice(&body).unwrap();
                        *capture_clone.lock().await = Some(val);
                    }
                }
                // Write a minimal success response.
                let resp = serde_json::json!({"jsonrpc":"2.0","id":"1","result":{}});
                let body = serde_json::to_vec(&resp).unwrap();
                let len = (body.len() as u32).to_le_bytes();
                let _ = conn.write_all(&len).await;
                let _ = conn.write_all(&body).await;
            }
        });

        let tools = vec![make_tool("list-prs"), make_tool("create-issue")];
        let names = register_tools_with_avix(&sock_path, "svc-token-abc", &tools, "github/")
            .await
            .unwrap();

        assert_eq!(names, vec!["github/list-prs", "github/create-issue"]);

        let captured = capture.lock().await;
        let req = captured.as_ref().unwrap();
        assert_eq!(req["method"], "ipc.tool-add");
        let tool_specs = req["params"]["tools"].as_array().unwrap();
        assert_eq!(tool_specs.len(), 2);
        assert_eq!(tool_specs[0]["name"], "github/list-prs");
        assert!(!tool_specs[0]["descriptor"]["streaming"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn handle_tool_call_routes_to_correct_server() {
        use crate::mcp_bridge::config::{McpServerConfig, McpTransport};
        use crate::mcp_bridge::connection::McpServerConnection;

        // We cannot construct a real McpServerConnection without spawning a process,
        // so we test the routing logic by checking the handler returns an error for
        // an unknown tool (since connections map is empty).
        let connections: Connections = Arc::new(RwLock::new(HashMap::new()));

        let req = crate::ipc::message::JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "test-1".into(),
            method: "github/list-prs".into(),
            params: serde_json::json!({}),
        };

        let resp = handle_tool_call(IpcMessage::Request(req), connections)
            .await
            .unwrap();

        assert!(resp.error.is_some());
        assert!(resp
            .error
            .unwrap()
            .message
            .contains("not found in any MCP server"));
    }

    #[tokio::test]
    async fn handle_tool_call_notification_returns_none() {
        let connections: Connections = Arc::new(RwLock::new(HashMap::new()));
        let notif = crate::ipc::message::JsonRpcNotification::new("ping", serde_json::json!({}));
        let resp = handle_tool_call(IpcMessage::Notification(notif), connections).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn deregister_empty_tools_is_noop() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("kernel.sock");
        // No socket bound — but with empty tools it should return Ok without connecting.
        let result = deregister_tools_from_avix(&sock_path, "svc-token-abc", &[], false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn register_empty_tools_is_noop() {
        let dir = TempDir::new().unwrap();
        let sock_path = dir.path().join("kernel.sock");
        let result =
            register_tools_with_avix(&sock_path, "svc-token-abc", &[], "mcp/github/").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
