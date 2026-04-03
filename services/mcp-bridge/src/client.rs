use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Information about a single tool exposed by an MCP server.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    /// The raw JSON Schema `inputSchema` from the MCP server, passed through verbatim.
    pub input_schema: Value,
}

/// Errors that can occur when communicating with an MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("MCP I/O error: {0}")]
    Io(String),
    #[error("MCP protocol error: {0}")]
    Protocol(String),
    #[error("MCP request timed out")]
    Timeout,
    #[error("MCP server process has gone away")]
    ServerGone,
}

impl From<McpClientError> for avix_core::error::AvixError {
    fn from(e: McpClientError) -> Self {
        match e {
            McpClientError::Io(s) => avix_core::error::AvixError::McpUnreachable(s),
            McpClientError::Protocol(s) => avix_core::error::AvixError::McpProtocol(s),
            McpClientError::Timeout => {
                avix_core::error::AvixError::McpUnreachable("request timed out".into())
            }
            McpClientError::ServerGone => {
                avix_core::error::AvixError::McpUnreachable("server process gone".into())
            }
        }
    }
}

/// Abstraction over the underlying MCP transport so tests can inject a mock.
#[async_trait]
pub trait McpTransportIO: Send + Sync {
    async fn send(&mut self, msg: Value) -> Result<(), McpClientError>;
    async fn recv(&mut self) -> Result<Value, McpClientError>;
}

// ── Stdio transport ───────────────────────────────────────────────────────────

/// Stdio transport: spawns a subprocess and communicates via its stdin/stdout
/// using newline-delimited JSON.
pub struct StdioTransport {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl StdioTransport {
    /// Spawn the MCP server subprocess.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, McpClientError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| McpClientError::Io(format!("failed to spawn '{command}': {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpClientError::Io("failed to get stdin handle".into()))?;
        let stdout_raw = child
            .stdout
            .take()
            .ok_or_else(|| McpClientError::Io("failed to get stdout handle".into()))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout_raw),
        })
    }
}

#[async_trait]
impl McpTransportIO for StdioTransport {
    async fn send(&mut self, msg: Value) -> Result<(), McpClientError> {
        let mut line =
            serde_json::to_string(&msg).map_err(|e| McpClientError::Protocol(e.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpClientError::Io(e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| McpClientError::Io(e.to_string()))
    }

    async fn recv(&mut self) -> Result<Value, McpClientError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| McpClientError::Io(e.to_string()))?;
        if n == 0 {
            return Err(McpClientError::ServerGone);
        }
        serde_json::from_str(line.trim()).map_err(|e| McpClientError::Protocol(e.to_string()))
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

// ── HTTP transport ────────────────────────────────────────────────────────────

/// HTTP transport: sends MCP JSON-RPC messages as POST requests.
pub struct HttpTransport {
    url: String,
    client: reqwest::Client,
    session_id: Option<String>,
}

impl HttpTransport {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
            session_id: None,
        }
    }
}

#[async_trait]
impl McpTransportIO for HttpTransport {
    async fn send(&mut self, msg: Value) -> Result<(), McpClientError> {
        let mut req = self.client.post(&self.url).json(&msg);
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| McpClientError::Io(e.to_string()))?;

        // Extract session id from response headers if present.
        if let Some(sid) = resp.headers().get("Mcp-Session-Id") {
            if let Ok(s) = sid.to_str() {
                self.session_id = Some(s.to_string());
            }
        }

        // Store the response body for the next `recv` call. Since HTTP is
        // synchronous request/response we return the body here and surface it
        // in `recv`. We use a temporary approach: stash the body in a thread-local
        // so `recv` can pick it up. For simplicity in this implementation we
        // do the full round-trip inside `send` and surface it via a pending buffer.
        // A cleaner implementation would use a channel; this is sufficient for v1.
        let body = resp
            .text()
            .await
            .map_err(|e| McpClientError::Io(e.to_string()))?;
        HTTP_PENDING_RESPONSE.with(|cell| {
            *cell.borrow_mut() = Some(body);
        });
        Ok(())
    }

    async fn recv(&mut self) -> Result<Value, McpClientError> {
        let body = HTTP_PENDING_RESPONSE.with(|cell| cell.borrow_mut().take());
        match body {
            Some(b) => {
                serde_json::from_str(&b).map_err(|e| McpClientError::Protocol(e.to_string()))
            }
            None => Err(McpClientError::Protocol(
                "recv called without pending HTTP response".into(),
            )),
        }
    }
}

thread_local! {
    static HTTP_PENDING_RESPONSE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

// ── McpClient ────────────────────────────────────────────────────────────────

/// High-level MCP client. Generic over the transport.
pub struct McpClient<T: McpTransportIO> {
    transport: T,
    next_id: u64,
    /// Buffer for notifications received while waiting for a response.
    notification_buffer: Vec<Value>,
}

impl<T: McpTransportIO> McpClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
            notification_buffer: Vec::new(),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send MCP `initialize` and wait for the server's `initialized` notification.
    pub async fn initialize(&mut self) -> Result<(), McpClientError> {
        let id = self.next_id();
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "avix-mcp-bridge",
                    "version": "0.1.0"
                }
            }
        });
        self.rpc_raw(id, req).await?;

        // Send `notifications/initialized` — fire and forget.
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.transport.send(notif).await?;
        Ok(())
    }

    /// Call `tools/list` and handle pagination via `nextCursor`.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, McpClientError> {
        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let id = self.next_id();
            let params = match &cursor {
                None => serde_json::json!({}),
                Some(c) => serde_json::json!({"cursor": c}),
            };
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/list",
                "params": params
            });
            let result = self.rpc_raw(id, req).await?;

            let tools_array = result
                .get("tools")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    McpClientError::Protocol("tools/list: missing 'tools' array".into())
                })?;

            for tool in tools_array {
                let name = tool
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpClientError::Protocol("tool missing 'name' field".into()))?
                    .to_string();
                let description = tool
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
                all_tools.push(McpToolInfo {
                    name,
                    description,
                    input_schema,
                });
            }

            // Check for next page.
            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if cursor.is_none() {
                break;
            }
        }

        Ok(all_tools)
    }

    /// Call `tools/call` and return the content array as a JSON value.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<Value, McpClientError> {
        let id = self.next_id();
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });
        let result = self.rpc_raw(id, req).await?;
        Ok(result)
    }

    /// Send a request and wait for the response matching `expected_id`.
    /// Responses not matching the expected id are buffered.
    async fn rpc_raw(&mut self, expected_id: u64, req: Value) -> Result<Value, McpClientError> {
        self.transport.send(req).await?;
        loop {
            let msg = self.transport.recv().await?;

            // Check if this is a notification (no "id" field) — buffer it.
            if msg.as_object().is_none_or(|o| !o.contains_key("id")) {
                self.notification_buffer.push(msg);
                continue;
            }

            // Check for error response.
            if let Some(error) = msg.get("error") {
                let msg_str = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(McpClientError::Protocol(msg_str.to_string()));
            }

            // Check id matches.
            let resp_id = msg.get("id").and_then(|v| v.as_u64());
            if resp_id == Some(expected_id) {
                return msg
                    .get("result")
                    .cloned()
                    .ok_or_else(|| McpClientError::Protocol("response missing 'result'".into()));
            }

            // Wrong id — keep waiting.
            self.notification_buffer.push(msg);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::sync::Mutex;

    /// A mock transport that replays canned JSON responses.
    struct MockTransport {
        responses: Mutex<VecDeque<Value>>,
        sent: Mutex<Vec<Value>>,
    }

    impl MockTransport {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                sent: Mutex::new(Vec::new()),
            }
        }

        async fn sent_messages(&self) -> Vec<Value> {
            self.sent.lock().await.clone()
        }
    }

    #[async_trait]
    impl McpTransportIO for MockTransport {
        async fn send(&mut self, msg: Value) -> Result<(), McpClientError> {
            self.sent.lock().await.push(msg);
            Ok(())
        }

        async fn recv(&mut self) -> Result<Value, McpClientError> {
            self.responses
                .lock()
                .await
                .pop_front()
                .ok_or(McpClientError::ServerGone)
        }
    }

    #[tokio::test]
    async fn initialize_sends_correct_protocol_version() {
        let transport = MockTransport::new(vec![
            // Response to initialize
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"test","version":"1.0"}}}),
        ]);
        let mut client = McpClient::new(transport);
        client.initialize().await.unwrap();

        let sent = client.transport.sent_messages().await;
        // First message is `initialize`
        assert_eq!(sent[0]["method"], "initialize");
        assert_eq!(sent[0]["params"]["protocolVersion"], "2024-11-05");
        // Second message is `notifications/initialized`
        assert_eq!(sent[1]["method"], "notifications/initialized");
    }

    #[tokio::test]
    async fn list_tools_returns_empty_for_empty_result() {
        let transport = MockTransport::new(vec![
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}}),
        ]);
        let mut client = McpClient::new(transport);
        let tools = client.list_tools().await.unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn list_tools_handles_pagination() {
        let transport = MockTransport::new(vec![
            // First page
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{
                "tools": [{"name":"list-prs","description":"List PRs","inputSchema":{}}],
                "nextCursor": "page2"
            }}),
            // Second page
            serde_json::json!({"jsonrpc":"2.0","id":2,"result":{
                "tools": [{"name":"create-issue","description":"Create issue","inputSchema":{}}]
            }}),
        ]);
        let mut client = McpClient::new(transport);
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "list-prs");
        assert_eq!(tools[1].name, "create-issue");
    }

    #[tokio::test]
    async fn list_tools_preserves_input_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "The repository name"}
            },
            "required": ["repo"]
        });
        let transport =
            MockTransport::new(vec![serde_json::json!({"jsonrpc":"2.0","id":1,"result":{
                "tools": [{"name":"list-prs","description":"List PRs","inputSchema": schema}]
            }})]);
        let mut client = McpClient::new(transport);
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools[0].input_schema, schema);
    }

    #[tokio::test]
    async fn call_tool_returns_text_content() {
        let transport =
            MockTransport::new(vec![serde_json::json!({"jsonrpc":"2.0","id":1,"result":{
                "content": [{"type":"text","text":"PR #1: Fix bug"}]
            }})]);
        let mut client = McpClient::new(transport);
        let result = client
            .call_tool("list-prs", serde_json::json!({"repo": "myrepo"}))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "PR #1: Fix bug");
    }

    #[tokio::test]
    async fn call_tool_error_response_returns_err() {
        let transport = MockTransport::new(vec![
            serde_json::json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Tool not found"}}),
        ]);
        let mut client = McpClient::new(transport);
        let result = client.call_tool("nonexistent", serde_json::json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            McpClientError::Protocol(msg) => assert!(msg.contains("Tool not found")),
            e => panic!("expected Protocol error, got {e:?}"),
        }
    }

    #[tokio::test]
    async fn rpc_buffers_notifications_and_matches_response() {
        // Inject a notification before the actual response.
        let transport = MockTransport::new(vec![
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/message","params":{}}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}}),
        ]);
        let mut client = McpClient::new(transport);
        let tools = client.list_tools().await.unwrap();
        assert!(tools.is_empty());
        // Notification was buffered.
        assert_eq!(client.notification_buffer.len(), 1);
    }

    #[tokio::test]
    async fn list_tools_second_page_cursor_sent() {
        let transport = MockTransport::new(vec![
            serde_json::json!({"jsonrpc":"2.0","id":1,"result":{
                "tools": [{"name":"tool-a","description":"A"}],
                "nextCursor": "cursor-xyz"
            }}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"tools":[]}}),
        ]);
        let mut client = McpClient::new(transport);
        client.list_tools().await.unwrap();

        let sent = client.transport.sent_messages().await;
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["params"]["cursor"], "cursor-xyz");
    }
}
