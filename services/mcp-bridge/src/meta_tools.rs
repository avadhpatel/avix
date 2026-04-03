//! Meta-tools exposed by the MCP bridge at the `mcp/` namespace.
//!
//! These tools provide discovery and diagnostic capabilities:
//!
//! | Tool name          | Description                                        |
//! |--------------------|----------------------------------------------------|
//! | `mcp/servers`      | List all configured MCP servers and their status   |
//! | `mcp/server-status`| Detailed status for one server (input: server_name)|
//! | `mcp/tools`        | List tools, optionally filtered by server          |
//!
//! All tools return YAML in the `apiVersion: avix/v1` manifest format.

use serde::{Deserialize, Serialize};

use avix_core::ipc::message::{JsonRpcRequest, JsonRpcResponse};
use crate::connection::McpServerConnection;

// ── Wire names ────────────────────────────────────────────────────────────────

pub const TOOL_SERVERS: &str = "mcp/servers";
pub const TOOL_SERVER_STATUS: &str = "mcp/server-status";
pub const TOOL_TOOLS: &str = "mcp/tools";

pub const ALL_META_TOOLS: &[&str] = &[TOOL_SERVERS, TOOL_SERVER_STATUS, TOOL_TOOLS];

// ── Descriptor JSON for ipc.tool-add ─────────────────────────────────────────

pub fn meta_tool_descriptors() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": TOOL_SERVERS,
            "descriptor": {
                "description": "List all configured MCP servers with their connection status, \
                                namespace, and tool count. Returns YAML (apiVersion: avix/v1, \
                                kind: McpServerList).",
                "streaming": false,
                "input_schema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            "visibility": "all"
        }),
        serde_json::json!({
            "name": TOOL_SERVER_STATUS,
            "descriptor": {
                "description": "Return detailed status for a single MCP server including \
                                connection state, tool list, and health configuration. \
                                Returns YAML (apiVersion: avix/v1, kind: McpServerStatus).",
                "streaming": false,
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "Name of the MCP server as declared in mcp.json"
                        }
                    },
                    "required": ["server_name"]
                }
            },
            "visibility": "all"
        }),
        serde_json::json!({
            "name": TOOL_TOOLS,
            "descriptor": {
                "description": "List tools exposed through the MCP bridge, optionally filtered \
                                to a single server. Returns YAML (apiVersion: avix/v1, \
                                kind: McpToolList).",
                "streaming": false,
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "server_name": {
                            "type": "string",
                            "description": "Optional — filter results to this server only"
                        }
                    },
                    "required": []
                }
            },
            "visibility": "all"
        }),
    ]
}

// ── YAML output types ─────────────────────────────────────────────────────────

/// mcp/servers output: `kind: McpServerList`
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerList {
    pub api_version: String,
    pub kind: String,
    pub spec: McpServerListSpec,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerListSpec {
    pub total: usize,
    pub servers: Vec<McpServerEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerEntry {
    pub name: String,
    pub namespace: String,
    pub status: String,
    pub tool_count: usize,
    pub health_check_interval_secs: u64,
}

/// mcp/server-status output: `kind: McpServerStatus`
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub api_version: String,
    pub kind: String,
    pub metadata: McpServerStatusMetadata,
    pub spec: McpServerStatusSpec,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusMetadata {
    pub name: String,
    pub namespace: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusSpec {
    pub status: String,
    pub tool_count: usize,
    pub health_check_interval_secs: u64,
    pub tools: Vec<McpToolEntry>,
}

/// mcp/tools output: `kind: McpToolList`
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolList {
    pub api_version: String,
    pub kind: String,
    pub spec: McpToolListSpec,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolListSpec {
    pub total: usize,
    pub tools: Vec<McpToolEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolEntry {
    /// Full Avix tool name, e.g. `"github/list-prs"`.
    pub name: String,
    pub description: String,
    pub server: String,
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Dispatch a meta-tool call. Returns `Some(response)` if the method is a
/// known meta-tool name, `None` if it should be handled elsewhere.
pub async fn handle_meta_tool(
    req: &JsonRpcRequest,
    connections: &tokio::sync::RwLock<std::collections::HashMap<String, McpServerConnection>>,
) -> Option<JsonRpcResponse> {
    match req.method.as_str() {
        TOOL_SERVERS => Some(handle_servers(req, connections).await),
        TOOL_SERVER_STATUS => Some(handle_server_status(req, connections).await),
        TOOL_TOOLS => Some(handle_tools(req, connections).await),
        _ => None,
    }
}

// ── mcp/servers ───────────────────────────────────────────────────────────────

async fn handle_servers(
    req: &JsonRpcRequest,
    connections: &tokio::sync::RwLock<std::collections::HashMap<String, McpServerConnection>>,
) -> JsonRpcResponse {
    let conns = connections.read().await;

    let mut servers: Vec<McpServerEntry> = conns
        .values()
        .map(|c| McpServerEntry {
            name: c.server_name().to_string(),
            namespace: c.namespace().to_string(),
            status: connection_status(c),
            tool_count: c.tools().len(),
            health_check_interval_secs: c.health_check_interval_secs(),
        })
        .collect();

    servers.sort_by(|a, b| a.name.cmp(&b.name));

    let manifest = McpServerList {
        api_version: "avix/v1".into(),
        kind: "McpServerList".into(),
        spec: McpServerListSpec {
            total: servers.len(),
            servers,
        },
    };

    yaml_response(&req.id, &manifest)
}

// ── mcp/server-status ─────────────────────────────────────────────────────────

async fn handle_server_status(
    req: &JsonRpcRequest,
    connections: &tokio::sync::RwLock<std::collections::HashMap<String, McpServerConnection>>,
) -> JsonRpcResponse {
    let server_name = match req.params.get("server_name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return JsonRpcResponse::err(
                &req.id,
                -32602,
                "missing required parameter: server_name",
                None,
            );
        }
    };

    let conns = connections.read().await;
    let conn = match conns.get(&server_name) {
        Some(c) => c,
        None => {
            return JsonRpcResponse::err(
                &req.id,
                -32001,
                &format!("MCP server '{}' not found", server_name),
                None,
            );
        }
    };

    let tools: Vec<McpToolEntry> = conn
        .tools()
        .iter()
        .map(|t| McpToolEntry {
            name: format!("{}{}", conn.namespace(), t.name),
            description: t.description.clone(),
            server: server_name.clone(),
        })
        .collect();

    let manifest = McpServerStatus {
        api_version: "avix/v1".into(),
        kind: "McpServerStatus".into(),
        metadata: McpServerStatusMetadata {
            name: server_name.clone(),
            namespace: conn.namespace().to_string(),
        },
        spec: McpServerStatusSpec {
            status: connection_status(conn),
            tool_count: tools.len(),
            health_check_interval_secs: conn.health_check_interval_secs(),
            tools,
        },
    };

    yaml_response(&req.id, &manifest)
}

// ── mcp/tools ─────────────────────────────────────────────────────────────────

async fn handle_tools(
    req: &JsonRpcRequest,
    connections: &tokio::sync::RwLock<std::collections::HashMap<String, McpServerConnection>>,
) -> JsonRpcResponse {
    let filter = req.params.get("server_name").and_then(|v| v.as_str());

    let conns = connections.read().await;

    // Validate filter if given.
    if let Some(name) = filter {
        if !conns.contains_key(name) {
            return JsonRpcResponse::err(
                &req.id,
                -32001,
                &format!("MCP server '{}' not found", name),
                None,
            );
        }
    }

    let mut tools: Vec<McpToolEntry> = conns
        .iter()
        .filter(|(name, _)| filter.is_none_or(|f| f == name.as_str()))
        .flat_map(|(server_name, conn)| {
            conn.tools().iter().map(move |t| McpToolEntry {
                name: format!("{}{}", conn.namespace(), t.name),
                description: t.description.clone(),
                server: server_name.clone(),
            })
        })
        .collect();

    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let manifest = McpToolList {
        api_version: "avix/v1".into(),
        kind: "McpToolList".into(),
        spec: McpToolListSpec {
            total: tools.len(),
            tools,
        },
    };

    yaml_response(&req.id, &manifest)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn connection_status(conn: &McpServerConnection) -> String {
    if conn.is_healthy() {
        "connected".into()
    } else if conn.is_degraded() {
        "degraded".into()
    } else {
        "disconnected".into()
    }
}

fn yaml_response<T: Serialize>(id: &str, value: &T) -> JsonRpcResponse {
    match serde_yaml::to_string(value) {
        Ok(yaml) => JsonRpcResponse::ok(id, serde_json::json!({ "content": yaml })),
        Err(e) => JsonRpcResponse::err(id, -32603, &format!("YAML serialization error: {e}"), None),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::sync::RwLock;

    fn make_connections(
        entries: Vec<(&str, &str, bool, Vec<(&str, &str)>)>,
        // (server_name, namespace, healthy, tools[(name, description)])
    ) -> RwLock<HashMap<String, McpServerConnection>> {
        use crate::client::McpToolInfo;
        use crate::config::{McpServerConfig, McpTransport};

        let mut map = HashMap::new();
        for (server_name, namespace, healthy, tool_pairs) in entries {
            let tools = tool_pairs
                .into_iter()
                .map(|(n, d)| McpToolInfo {
                    name: n.to_string(),
                    description: d.to_string(),
                    input_schema: serde_json::json!({}),
                })
                .collect();
            let cfg = McpServerConfig {
                command: "true".into(),
                args: vec![],
                env: HashMap::new(),
                mount: None,
                transport: McpTransport::Stdio,
                health_check_interval_secs: 30,
            };
            let conn =
                McpServerConnection::new_for_test(server_name, namespace, cfg, tools, healthy);
            map.insert(server_name.to_string(), conn);
        }
        RwLock::new(map)
    }

    fn make_req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: "test-1".into(),
            method: method.into(),
            params,
        }
    }

    // ── mcp/servers ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn servers_returns_yaml_manifest() {
        let conns = make_connections(vec![(
            "github",
            "github/",
            true,
            vec![("list-prs", "List PRs")],
        )]);
        let req = make_req(TOOL_SERVERS, serde_json::json!({}));
        let resp = handle_servers(&req, &conns).await;
        assert!(resp.error.is_none());
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("apiVersion: avix/v1"));
        assert!(yaml.contains("McpServerList"));
        assert!(yaml.contains("github"));
    }

    #[tokio::test]
    async fn servers_empty_returns_zero_total() {
        let conns = make_connections(vec![]);
        let req = make_req(TOOL_SERVERS, serde_json::json!({}));
        let resp = handle_servers(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("total: 0"));
    }

    #[tokio::test]
    async fn servers_status_reflects_health() {
        let conns = make_connections(vec![
            ("github", "github/", true, vec![]),
            ("slack", "slack/", false, vec![]),
        ]);
        let req = make_req(TOOL_SERVERS, serde_json::json!({}));
        let resp = handle_servers(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("connected"));
        assert!(yaml.contains("disconnected"));
    }

    #[tokio::test]
    async fn servers_tool_count_correct() {
        let conns = make_connections(vec![(
            "github",
            "github/",
            true,
            vec![("list-prs", ""), ("create-issue", ""), ("merge-pr", "")],
        )]);
        let req = make_req(TOOL_SERVERS, serde_json::json!({}));
        let resp = handle_servers(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("toolCount: 3"));
    }

    // ── mcp/server-status ────────────────────────────────────────────────────

    #[tokio::test]
    async fn server_status_returns_manifest_for_known_server() {
        let conns = make_connections(vec![(
            "github",
            "github/",
            true,
            vec![("list-prs", "List open PRs")],
        )]);
        let req = make_req(
            TOOL_SERVER_STATUS,
            serde_json::json!({"server_name": "github"}),
        );
        let resp = handle_server_status(&req, &conns).await;
        assert!(resp.error.is_none());
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("McpServerStatus"));
        assert!(yaml.contains("github"));
        assert!(yaml.contains("list-prs"));
        assert!(yaml.contains("connected"));
    }

    #[tokio::test]
    async fn server_status_missing_param_returns_error() {
        let conns = make_connections(vec![]);
        let req = make_req(TOOL_SERVER_STATUS, serde_json::json!({}));
        let resp = handle_server_status(&req, &conns).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn server_status_unknown_server_returns_error() {
        let conns = make_connections(vec![]);
        let req = make_req(
            TOOL_SERVER_STATUS,
            serde_json::json!({"server_name": "nonexistent"}),
        );
        let resp = handle_server_status(&req, &conns).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32001);
    }

    #[tokio::test]
    async fn server_status_includes_full_tool_names() {
        let conns = make_connections(vec![("github", "github/", true, vec![("list-prs", "")])]);
        let req = make_req(
            TOOL_SERVER_STATUS,
            serde_json::json!({"server_name": "github"}),
        );
        let resp = handle_server_status(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            yaml.contains("github/list-prs"),
            "expected full Avix tool name in output"
        );
    }

    // ── mcp/tools ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn tools_lists_all_tools_when_no_filter() {
        let conns = make_connections(vec![
            (
                "github",
                "github/",
                true,
                vec![("list-prs", ""), ("create-issue", "")],
            ),
            ("slack", "slack/", true, vec![("send-message", "")]),
        ]);
        let req = make_req(TOOL_TOOLS, serde_json::json!({}));
        let resp = handle_tools(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("McpToolList"));
        assert!(yaml.contains("github/list-prs"));
        assert!(yaml.contains("github/create-issue"));
        assert!(yaml.contains("slack/send-message"));
        assert!(yaml.contains("total: 3"));
    }

    #[tokio::test]
    async fn tools_filters_by_server_name() {
        let conns = make_connections(vec![
            ("github", "github/", true, vec![("list-prs", "")]),
            ("slack", "slack/", true, vec![("send-message", "")]),
        ]);
        let req = make_req(TOOL_TOOLS, serde_json::json!({"server_name": "github"}));
        let resp = handle_tools(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(yaml.contains("github/list-prs"));
        assert!(!yaml.contains("slack/send-message"));
        assert!(yaml.contains("total: 1"));
    }

    #[tokio::test]
    async fn tools_unknown_server_filter_returns_error() {
        let conns = make_connections(vec![]);
        let req = make_req(
            TOOL_TOOLS,
            serde_json::json!({"server_name": "nonexistent"}),
        );
        let resp = handle_tools(&req, &conns).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32001);
    }

    #[tokio::test]
    async fn tools_output_is_sorted_by_name() {
        let conns = make_connections(vec![(
            "github",
            "github/",
            true,
            vec![("merge-pr", ""), ("create-issue", ""), ("list-prs", "")],
        )]);
        let req = make_req(TOOL_TOOLS, serde_json::json!({}));
        let resp = handle_tools(&req, &conns).await;
        let yaml = resp.result.unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        let create_pos = yaml.find("create-issue").unwrap();
        let list_pos = yaml.find("list-prs").unwrap();
        let merge_pos = yaml.find("merge-pr").unwrap();
        assert!(
            create_pos < list_pos && list_pos < merge_pos,
            "tools should be sorted"
        );
    }

    // ── handle_meta_tool dispatch ─────────────────────────────────────────────

    #[tokio::test]
    async fn handle_meta_tool_returns_none_for_unknown_method() {
        let conns = make_connections(vec![]);
        let req = make_req("github/list-prs", serde_json::json!({}));
        let result = handle_meta_tool(&req, &conns).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn handle_meta_tool_dispatches_servers() {
        let conns = make_connections(vec![]);
        let req = make_req(TOOL_SERVERS, serde_json::json!({}));
        let result = handle_meta_tool(&req, &conns).await;
        assert!(result.is_some());
        assert!(result.unwrap().error.is_none());
    }
}
