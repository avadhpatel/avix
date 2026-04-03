use std::path::PathBuf;

use tracing::info;
use tracing_subscriber::EnvFilter;

use avix_core::error::AvixError;
use avix_core::mcp_bridge::{McpBridgeRunner, McpConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mcp_bridge=info".parse()?))
        .init();

    let kernel_sock = std::env::var("AVIX_KERNEL_SOCK")
        .map(PathBuf::from)
        .map_err(|_| AvixError::ConfigParse("AVIX_KERNEL_SOCK not set".into()))?;

    let svc_sock = std::env::var("AVIX_SVC_SOCK")
        .map(PathBuf::from)
        .map_err(|_| AvixError::ConfigParse("AVIX_SVC_SOCK not set".into()))?;

    let token = std::env::var("AVIX_SVC_TOKEN")
        .map_err(|_| AvixError::ConfigParse("AVIX_SVC_TOKEN not set".into()))?;

    let root = std::env::var("AVIX_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/avix"));

    let mcp_json_path = root.join("etc/mcp.json");
    let config = McpConfig::load(&mcp_json_path)?;

    if config.mcp_servers.is_empty() {
        info!("no MCP servers configured — exiting");
        return Ok(());
    }

    let runner = McpBridgeRunner::new(config, kernel_sock, token, svc_sock);
    let _bridge = runner.start().await?;

    tokio::signal::ctrl_c().await?;
    Ok(())
}
