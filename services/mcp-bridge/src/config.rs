use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use avix_core::error::AvixError;

fn default_health_check_interval() -> u64 {
    30
}

/// Top-level `/etc/avix/mcp.json` configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

impl McpConfig {
    /// Load and parse `/etc/avix/mcp.json` from `path`.
    pub fn load(path: &Path) -> Result<Self, AvixError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AvixError::NotFound(format!("cannot read {}: {e}", path.display())))?;
        serde_json::from_str(&content)
            .map_err(|e| AvixError::ConfigParse(format!("mcp.json parse error: {e}")))
    }

    /// Return an empty config (no servers).
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Configuration for a single MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables. Values prefixed with `"secret:<NAME>"` are resolved at
    /// connection time by looking up `<NAME>` in the Avix secret store.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional Avix-specific mount override, e.g. `"/tools/github"`.
    /// If absent, defaults to `/tools/mcp/<server_name>`.
    pub mount: Option<String>,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default = "default_health_check_interval")]
    pub health_check_interval_secs: u64,
}

impl McpServerConfig {
    /// Resolve the Avix tool namespace prefix for this server.
    ///
    /// Examples:
    /// - `mount = None`, `server_name = "github"` → `"mcp/github/"`
    /// - `mount = "/tools/github"` → `"github/"`
    /// - `mount = "/tools/mcp/foo"` → `"mcp/foo/"`
    /// - `mount = "/tools/google-workspace"` → `"google-workspace/"`
    pub fn tool_namespace(&self, server_name: &str) -> String {
        match &self.mount {
            None => format!("{}/", server_name),
            Some(mount) => {
                // Strip the leading "/tools/" prefix to get the namespace.
                let stripped = mount
                    .strip_prefix("/tools/")
                    .unwrap_or(mount.trim_start_matches('/'));
                // Ensure trailing slash.
                if stripped.ends_with('/') {
                    stripped.to_string()
                } else {
                    format!("{}/", stripped)
                }
            }
        }
    }
}

/// MCP server transport type.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http {
        url: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_json(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn config_parses_minimal_stdio_server() {
        let f = write_json(
            r#"{"mcpServers": {"github": {"command": "uvx", "args": ["mcp-server-github"]}}}"#,
        );
        let cfg = McpConfig::load(f.path()).unwrap();
        let svc = cfg.mcp_servers.get("github").unwrap();
        assert_eq!(svc.command, "uvx");
        assert_eq!(svc.args, vec!["mcp-server-github"]);
        assert_eq!(svc.transport, McpTransport::Stdio);
        assert_eq!(svc.health_check_interval_secs, 30);
    }

    #[test]
    fn config_empty_servers_object_ok() {
        let f = write_json(r#"{"mcpServers": {}}"#);
        let cfg = McpConfig::load(f.path()).unwrap();
        assert!(cfg.mcp_servers.is_empty());
    }

    #[test]
    fn config_missing_file_returns_error() {
        let result = McpConfig::load(Path::new("/nonexistent/mcp.json"));
        assert!(result.is_err());
        match result.unwrap_err() {
            AvixError::NotFound(_) => {}
            e => panic!("expected NotFound, got {e:?}"),
        }
    }

    #[test]
    fn config_env_secret_reference_preserved() {
        let f = write_json(
            r#"{"mcpServers": {"github": {"command": "uvx", "env": {"GITHUB_TOKEN": "secret:GITHUB_TOKEN"}}}}"#,
        );
        let cfg = McpConfig::load(f.path()).unwrap();
        let svc = cfg.mcp_servers.get("github").unwrap();
        assert_eq!(svc.env.get("GITHUB_TOKEN").unwrap(), "secret:GITHUB_TOKEN");
    }

    #[test]
    fn config_http_transport_parses() {
        let f = write_json(
            r#"{"mcpServers": {"myserver": {"command": "ignored", "transport": {"type": "http", "url": "http://localhost:8080"}}}}"#,
        );
        let cfg = McpConfig::load(f.path()).unwrap();
        let svc = cfg.mcp_servers.get("myserver").unwrap();
        assert_eq!(
            svc.transport,
            McpTransport::Http {
                url: "http://localhost:8080".to_string()
            }
        );
    }

    #[test]
    fn tool_namespace_default_uses_server_name_directly() {
        let svc = McpServerConfig {
            command: "uvx".into(),
            args: vec![],
            env: HashMap::new(),
            mount: None,
            transport: McpTransport::Stdio,
            health_check_interval_secs: 30,
        };
        assert_eq!(svc.tool_namespace("github"), "github/");
    }

    #[test]
    fn tool_namespace_custom_mount_strips_tools_prefix() {
        let svc = McpServerConfig {
            command: "uvx".into(),
            args: vec![],
            env: HashMap::new(),
            mount: Some("/tools/github".to_string()),
            transport: McpTransport::Stdio,
            health_check_interval_secs: 30,
        };
        assert_eq!(svc.tool_namespace("github"), "github/");
    }

    #[test]
    fn tool_namespace_deep_path() {
        let svc = McpServerConfig {
            command: "uvx".into(),
            args: vec![],
            env: HashMap::new(),
            mount: Some("/tools/google-workspace".to_string()),
            transport: McpTransport::Stdio,
            health_check_interval_secs: 30,
        };
        assert_eq!(svc.tool_namespace("google-workspace"), "google-workspace/");
    }

    #[test]
    fn tool_namespace_explicit_mcp_mount_preserved() {
        let svc = McpServerConfig {
            command: "uvx".into(),
            args: vec![],
            env: HashMap::new(),
            mount: Some("/tools/mcp/foo".to_string()),
            transport: McpTransport::Stdio,
            health_check_interval_secs: 30,
        };
        assert_eq!(svc.tool_namespace("foo"), "mcp/foo/");
    }

    #[test]
    fn tool_namespace_no_double_trailing_slash() {
        let svc = McpServerConfig {
            command: "uvx".into(),
            args: vec![],
            env: HashMap::new(),
            mount: Some("/tools/github/".to_string()),
            transport: McpTransport::Stdio,
            health_check_interval_secs: 30,
        };
        let ns = svc.tool_namespace("github");
        assert!(!ns.contains("//"), "no double slash: {ns}");
        assert!(ns.ends_with('/'));
    }

    #[test]
    fn config_with_custom_mount() {
        let f = write_json(
            r#"{"mcpServers": {"github": {"command": "uvx", "mount": "/tools/github"}}}"#,
        );
        let cfg = McpConfig::load(f.path()).unwrap();
        let svc = cfg.mcp_servers.get("github").unwrap();
        assert_eq!(svc.mount.as_deref(), Some("/tools/github"));
        assert_eq!(svc.tool_namespace("github"), "github/");
    }
}
