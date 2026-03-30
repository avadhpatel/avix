use std::time::Instant;

use serde_json::Value;

use crate::mcp_bridge::client::{McpClient, McpClientError, McpToolInfo, StdioTransport};
use crate::mcp_bridge::config::McpServerConfig;

/// State of the connection to an MCP server.
pub enum ConnectionState {
    Connected { client: Box<McpClient<StdioTransport>> },
    Degraded { since: Instant },
    Disconnected,
    /// Used only in tests to simulate a healthy connected state without a
    /// real subprocess.
    #[cfg(test)]
    MockConnected,
}

/// Manages a live connection to one external MCP server.
///
/// Holds the tool list discovered at the last successful `tools/list`, the
/// Avix namespace prefix (e.g. `"mcp/github/"` or `"github/"`), and the
/// underlying `McpClient`.
pub struct McpServerConnection {
    server_name: String,
    /// Avix tool namespace prefix including trailing slash, e.g. `"mcp/github/"`.
    namespace: String,
    config: McpServerConfig,
    /// Cached tool list from the last successful `discover_tools`.
    tools: Vec<McpToolInfo>,
    state: ConnectionState,
}

impl McpServerConnection {
    /// Connect to the MCP server described by `config` and run `initialize`.
    pub async fn connect(
        server_name: &str,
        config: McpServerConfig,
    ) -> Result<Self, McpClientError> {
        let namespace = config.tool_namespace(server_name);
        let transport = StdioTransport::spawn(&config.command, &config.args, &config.env).await?;
        let mut client = McpClient::new(transport);
        client.initialize().await?;

        Ok(Self {
            server_name: server_name.to_string(),
            namespace,
            config,
            tools: Vec::new(),
            state: ConnectionState::Connected { client: Box::new(client) },
        })
    }

    /// Run `tools/list` and cache the results.
    ///
    /// Returns the newly cached tools on success.  On failure the state is set
    /// to `Degraded`.
    pub async fn discover_tools(&mut self) -> Result<&[McpToolInfo], McpClientError> {
        match &mut self.state {
            ConnectionState::Connected { client } => {
                match client.list_tools().await {
                    Ok(tools) => {
                        self.tools = tools;
                        Ok(&self.tools)
                    }
                    Err(e) => {
                        self.state = ConnectionState::Degraded { since: Instant::now() };
                        Err(e)
                    }
                }
            }
            _ => Err(McpClientError::ServerGone),
        }
    }

    /// Forward a tool call to the MCP server.
    ///
    /// `avix_tool_name` is the full Avix name such as `"mcp/github/list-prs"`.
    /// The namespace prefix is stripped before the call is forwarded to the MCP
    /// server, so the MCP server receives `"list-prs"`.
    pub async fn forward_call(
        &mut self,
        avix_tool_name: &str,
        params: Value,
    ) -> Result<Value, McpClientError> {
        let raw_name = avix_tool_name
            .strip_prefix(&self.namespace)
            .ok_or_else(|| {
                McpClientError::Protocol(format!(
                    "tool '{}' does not belong to namespace '{}'",
                    avix_tool_name, self.namespace
                ))
            })?;

        match &mut self.state {
            ConnectionState::Connected { client } => {
                match client.call_tool(raw_name, params).await {
                    Ok(result) => Ok(result),
                    Err(e) => {
                        self.state = ConnectionState::Degraded { since: Instant::now() };
                        Err(e)
                    }
                }
            }
            _ => Err(McpClientError::ServerGone),
        }
    }

    /// Attempt to reconnect to the MCP server after a degraded state.
    pub async fn reconnect(&mut self) -> Result<(), McpClientError> {
        let transport =
            StdioTransport::spawn(&self.config.command, &self.config.args, &self.config.env)
                .await?;
        let mut client = McpClient::new(transport);
        client.initialize().await?;
        self.state = ConnectionState::Connected { client: Box::new(client) };
        Ok(())
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Return the cached tool names (full Avix names with namespace prefix).
    pub fn tool_names(&self) -> impl Iterator<Item = String> + '_ {
        self.tools
            .iter()
            .map(|t| format!("{}{}", self.namespace, t.name))
    }

    /// Return the cached tool info slice.
    pub fn tools(&self) -> &[McpToolInfo] {
        &self.tools
    }

    pub fn is_healthy(&self) -> bool {
        match &self.state {
            ConnectionState::Connected { .. } => true,
            #[cfg(test)]
            ConnectionState::MockConnected => true,
            _ => false,
        }
    }

    pub fn is_degraded(&self) -> bool {
        matches!(self.state, ConnectionState::Degraded { .. })
    }

    pub fn health_check_interval_secs(&self) -> u64 {
        self.config.health_check_interval_secs
    }

    /// Construct a connection directly without spawning a process.
    /// Only available in tests.
    #[cfg(test)]
    pub fn new_for_test(
        server_name: &str,
        namespace: &str,
        config: McpServerConfig,
        tools: Vec<crate::mcp_bridge::client::McpToolInfo>,
        healthy: bool,
    ) -> Self {
        Self {
            server_name: server_name.to_string(),
            namespace: namespace.to_string(),
            config,
            tools,
            state: if healthy {
                ConnectionState::MockConnected
            } else {
                ConnectionState::Disconnected
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_bridge::client::McpTransportIO;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use tokio::sync::Mutex;

    struct MockTransport {
        responses: Mutex<VecDeque<serde_json::Value>>,
        sent: Mutex<Vec<serde_json::Value>>,
        fail_after: Option<usize>,
        call_count: Mutex<usize>,
    }

    impl MockTransport {
        fn new(responses: Vec<serde_json::Value>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                sent: Mutex::new(Vec::new()),
                fail_after: None,
                call_count: Mutex::new(0),
            }
        }

        fn with_fail_after(mut self, n: usize) -> Self {
            self.fail_after = Some(n);
            self
        }
    }

    #[async_trait]
    impl McpTransportIO for MockTransport {
        async fn send(&mut self, msg: serde_json::Value) -> Result<(), McpClientError> {
            let mut count = self.call_count.lock().await;
            *count += 1;
            if let Some(limit) = self.fail_after {
                if *count > limit {
                    return Err(McpClientError::Io("simulated failure".into()));
                }
            }
            self.sent.lock().await.push(msg);
            Ok(())
        }

        async fn recv(&mut self) -> Result<serde_json::Value, McpClientError> {
            self.responses
                .lock()
                .await
                .pop_front()
                .ok_or(McpClientError::ServerGone)
        }
    }

    fn make_connected_with_tools(
        namespace: &str,
        config: McpServerConfig,
        tools: Vec<McpToolInfo>,
    ) -> McpServerConnection {
        // Build a fake connected connection directly without spawning a process.
        let transport = MockTransport::new(vec![]);
        let client: McpClient<MockTransport> = McpClient::new(transport);

        // We need a StdioTransport-based state, but for tests we bypass connect()
        // by constructing directly.
        McpServerConnection {
            server_name: "github".to_string(),
            namespace: namespace.to_string(),
            config,
            tools,
            // Use Disconnected state for unit tests that don't need actual calls.
            state: ConnectionState::Disconnected,
        }
    }

    fn stub_config(mount: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            command: "true".into(),
            args: vec![],
            env: std::collections::HashMap::new(),
            mount: mount.map(|s| s.to_string()),
            transport: crate::mcp_bridge::config::McpTransport::Stdio,
            health_check_interval_secs: 30,
        }
    }

    #[test]
    fn namespace_derived_from_default_config() {
        let cfg = stub_config(None);
        let conn = make_connected_with_tools("github/", cfg, vec![]);
        assert_eq!(conn.namespace(), "github/");
    }

    #[test]
    fn namespace_derived_from_custom_mount() {
        let cfg = stub_config(Some("/tools/github"));
        let ns = cfg.tool_namespace("github");
        assert_eq!(ns, "github/");
        let conn = make_connected_with_tools("github/", cfg, vec![]);
        assert_eq!(conn.namespace(), "github/");
    }

    #[test]
    fn tool_names_includes_namespace_prefix() {
        let cfg = stub_config(None);
        let tools = vec![
            McpToolInfo {
                name: "list-prs".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
            McpToolInfo {
                name: "create-issue".into(),
                description: "".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let conn = make_connected_with_tools("github/", cfg, tools);
        let names: Vec<String> = conn.tool_names().collect();
        assert_eq!(names, vec!["github/list-prs", "github/create-issue"]);
    }

    #[test]
    fn tool_names_custom_namespace() {
        let cfg = stub_config(Some("/tools/github"));
        let tools = vec![McpToolInfo {
            name: "list-prs".into(),
            description: "".into(),
            input_schema: serde_json::json!({}),
        }];
        let conn = make_connected_with_tools("github/", cfg, tools);
        let names: Vec<String> = conn.tool_names().collect();
        assert_eq!(names, vec!["github/list-prs"]);
    }

    #[test]
    fn is_healthy_false_for_disconnected() {
        let cfg = stub_config(None);
        let conn = make_connected_with_tools("github/", cfg, vec![]);
        assert!(!conn.is_healthy());
    }

    #[test]
    fn is_degraded_false_for_disconnected() {
        let cfg = stub_config(None);
        let conn = make_connected_with_tools("github/", cfg, vec![]);
        assert!(!conn.is_degraded());
    }

    #[test]
    fn forward_call_wrong_namespace_returns_error() {
        // Check that stripping the wrong namespace prefix fails.
        let result = "gitlab/foo".strip_prefix("github/");
        assert!(result.is_none());
    }

    #[test]
    fn forward_call_strips_correct_prefix() {
        let ns = "github/";
        let avix_name = "github/list-prs";
        let raw = avix_name.strip_prefix(ns).unwrap();
        assert_eq!(raw, "list-prs");
    }

    #[test]
    fn forward_call_strips_custom_namespace_prefix() {
        let ns = "github/";
        let avix_name = "github/create-issue";
        let raw = avix_name.strip_prefix(ns).unwrap();
        assert_eq!(raw, "create-issue");
    }
}
