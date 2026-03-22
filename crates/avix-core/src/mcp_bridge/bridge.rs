use crate::error::AvixError;
use crate::types::tool::ToolName;

#[derive(Debug, Clone)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    pub server: String,
}

pub struct McpBridge {
    server_name: String,
    tools: Vec<McpToolDescriptor>,
}

impl McpBridge {
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            tools: Vec::new(),
        }
    }

    /// Register a raw MCP tool name (e.g. "create-issue") with prefix
    pub fn register_tool(
        &mut self,
        raw_name: &str,
        description: &str,
    ) -> Result<McpToolDescriptor, AvixError> {
        let namespaced = format!("mcp/{}/{}", self.server_name, raw_name);
        // Validate the name
        ToolName::parse(&namespaced)?;
        let desc = McpToolDescriptor {
            name: namespaced,
            description: description.to_string(),
            server: self.server_name.clone(),
        };
        self.tools.push(desc.clone());
        Ok(desc)
    }

    pub fn tools(&self) -> &[McpToolDescriptor] {
        &self.tools
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Strip the mcp/<server>/ prefix to get the raw outbound tool name
    pub fn outbound_name(tool_name: &str, server: &str) -> Option<String> {
        let prefix = format!("mcp/{}/", server);
        tool_name.strip_prefix(&prefix).map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_gets_mcp_prefix() {
        let mut bridge = McpBridge::new("github");
        let desc = bridge
            .register_tool("create-issue", "Creates a GitHub issue")
            .unwrap();
        assert_eq!(desc.name, "mcp/github/create-issue");
    }

    #[test]
    fn test_tool_count() {
        let mut bridge = McpBridge::new("github");
        bridge.register_tool("list-prs", "List PRs").unwrap();
        bridge
            .register_tool("create-issue", "Create issue")
            .unwrap();
        assert_eq!(bridge.tool_count(), 2);
    }

    #[test]
    fn test_outbound_name_strips_prefix() {
        let raw = McpBridge::outbound_name("mcp/github/create-issue", "github");
        assert_eq!(raw, Some("create-issue".to_string()));
    }

    #[test]
    fn test_outbound_name_wrong_server() {
        let raw = McpBridge::outbound_name("mcp/github/create-issue", "gitlab");
        assert!(raw.is_none());
    }

    #[test]
    fn test_server_name_preserved() {
        let mut bridge = McpBridge::new("slack");
        let desc = bridge
            .register_tool("send-message", "Send a message")
            .unwrap();
        assert_eq!(desc.server, "slack");
    }

    #[test]
    fn test_tools_listed() {
        let mut bridge = McpBridge::new("github");
        bridge.register_tool("list-prs", "List PRs").unwrap();
        let tools = bridge.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp/github/list-prs");
    }

    #[test]
    fn test_multiple_servers_independent() {
        let mut bridge1 = McpBridge::new("github");
        let mut bridge2 = McpBridge::new("slack");
        bridge1.register_tool("list-prs", "List PRs").unwrap();
        bridge2.register_tool("send-message", "Send msg").unwrap();
        assert_eq!(bridge1.tool_count(), 1);
        assert_eq!(bridge2.tool_count(), 1);
        assert_eq!(bridge1.tools()[0].name, "mcp/github/list-prs");
        assert_eq!(bridge2.tools()[0].name, "mcp/slack/send-message");
    }

    #[test]
    fn test_namespaced_tool_valid_name() {
        let mut bridge = McpBridge::new("jira");
        let result = bridge.register_tool("create-ticket", "Create a ticket");
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert_eq!(desc.name, "mcp/jira/create-ticket");
    }

    #[test]
    fn test_description_preserved() {
        let mut bridge = McpBridge::new("github");
        let desc = bridge
            .register_tool("list-prs", "List all open pull requests")
            .unwrap();
        assert_eq!(desc.description, "List all open pull requests");
    }

    #[test]
    fn test_outbound_name_with_nested_path() {
        let raw = McpBridge::outbound_name("mcp/github/org/list-repos", "github");
        assert_eq!(raw, Some("org/list-repos".to_string()));
    }

    #[test]
    fn test_zero_tools_initially() {
        let bridge = McpBridge::new("github");
        assert_eq!(bridge.tool_count(), 0);
    }
}
