# MCP Bridge Service — Gap A: Full Implementation

## Context

Avix needs a bridge service that connects external MCP (Model Context Protocol) servers to
the Avix tool registry. Agents call tools from MCP servers (GitHub, Google Workspace, etc.)
via the standard Avix path `/tools/<namespace>/`. The bridge reads `/etc/avix/mcp.json` at
startup, connects to each configured MCP server, discovers their tools, registers them into
the tool registry, and forwards incoming tool calls to the appropriate MCP server.

## Architecture

```
Agent → router.svc → mcp-bridge IpcServer
                           ↓ (namespace lookup)
               McpServerConnection.forward_call()
                           ↓ (tools/call JSON-RPC)
               External MCP server (stdio / HTTP)
```

**Mount path logic:**
- Default: server name `"github"` → namespace `mcp/github/` → VFS `/tools/mcp/github`
- Custom `"mount": "/tools/github"` → namespace `github/` → VFS `/tools/github`

## Implementation Steps (TDD order)

### Step 1 — MCP config types
**New:** `crates/avix-core/src/mcp_bridge/config.rs`

`McpConfig`, `McpServerConfig`, `McpTransport`. `tool_namespace()` converts mount path to
Avix tool namespace prefix. `McpConfig::load(path)` reads JSON from disk.

### Step 2 — Error variants
**Modify:** `crates/avix-core/src/error.rs`

Add `McpProtocol(String)`, `McpUnreachable(String)`.
Add `impl From<McpClientError> for AvixError`.

### Step 3 — MCP protocol client
**New:** `crates/avix-core/src/mcp_bridge/client.rs`

`McpTransportIO` trait, `StdioTransport`, `HttpTransport`, `McpClient<T>`.
Implements `initialize`, `list_tools` (with pagination), `call_tool`.
All tests use `MockTransport`.

### Step 4 — Server connection wrapper
**New:** `crates/avix-core/src/mcp_bridge/connection.rs`

`McpServerConnection` wraps `McpClient`, holds tool list, `ConnectionState`.
`forward_call` strips namespace prefix before delegating.

### Step 5 — Bridge runner
**New:** `crates/avix-core/src/mcp_bridge/runner.rs`

`McpBridgeRunner::start()`: connect → discover → `ipc.tool-add` → bind IpcServer → health monitor.
`RunningBridge::shutdown()`: cancel server → `ipc.tool-remove` → stop health monitor.

### Step 6 — Module exports
**Modify:** `crates/avix-core/src/mcp_bridge/mod.rs`

Export all new types.

### Step 7 — Config init
**Modify:** `crates/avix-core/src/cli/config_init.rs`

Write empty `mcp.json` during `avix config init`.

### Step 8 — Service unit
**New:** `services/mcp-bridge/service.unit`

TOML service definition for `mcp-bridge.svc`.

### Step 9 — Bootstrap integration
**Modify:** `crates/avix-core/src/bootstrap/mod.rs`

Start `McpBridgeRunner` in `phase3_services()` if `mcp.json` is non-empty. Non-fatal on failure.

## Verification

```bash
cargo test --workspace
cargo test -p avix-core mcp_bridge
cargo clippy --workspace -- -D warnings
```
