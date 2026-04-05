use anyhow::{Context, Result};
use clap::Subcommand;

use avix_client_core::atp::types::Cmd as AtpCmd_;
use avix_client_core::commands::spawn_agent::spawn_agent;
use avix_client_core::commands::{
    get_invocation, kill_agent, list_agents, list_installed, list_invocations,
    list_invocations_live, snapshot_invocation,
};
use avix_core::service::package_source::PackageSource;

use crate::util::{connect_config, emit, format_catalog, format_history, format_invocation};

#[derive(Subcommand)]
pub enum AgentCmd {
    /// Spawn a new agent
    Spawn {
        /// Agent name
        name: String,
        /// Goal for the agent
        #[arg(long)]
        goal: String,
        /// Capabilities (comma-separated)
        #[arg(long, value_delimiter = ',')]
        capabilities: Vec<String>,
    },
    /// List active agents
    List,
    /// Kill an agent by PID
    Kill {
        /// PID of the agent
        pid: u64,
    },
    /// List installed agents available to the current user
    Catalog {
        /// Username to query (defaults to current user)
        #[arg(long)]
        username: Option<String>,
    },
    /// List invocation history
    History {
        /// Filter by agent name
        #[arg(long)]
        agent: Option<String>,
        /// Username to query (defaults to current user)
        #[arg(long)]
        username: Option<String>,
        /// Include currently-running invocations
        #[arg(long)]
        live: bool,
    },
    /// Show a specific invocation (summary + conversation)
    Show {
        /// Invocation ID
        invocation_id: String,
    },
    /// Force an immediate snapshot of a running invocation
    Snapshot {
        /// Invocation ID to snapshot
        invocation_id: String,
    },
    /// Install an agent from a local path, URL, or GitHub source
    Install {
        /// Package source — local path, https:// URL, or github:owner/repo/name
        source: String,
        /// Install scope: `user` (default) or `system`
        #[arg(long, default_value = "user")]
        scope: String,
        /// Specific version or tag (default: latest)
        #[arg(long)]
        version: Option<String>,
        /// Expected checksum in "sha256:<hex>" format
        #[arg(long)]
        checksum: Option<String>,
        /// Skip checksum verification (trusted local dev only)
        #[arg(long)]
        no_verify: bool,
        /// Log this install under a specific session ID
        #[arg(long)]
        session: Option<String>,
        /// Print what would happen without actually installing
        #[arg(long)]
        dry_run: bool,
    },
    /// Uninstall an installed agent
    Uninstall {
        /// Agent name to uninstall
        name: String,
        /// Install scope: `user` (default) or `system`
        #[arg(long, default_value = "user")]
        scope: String,
    },
}

pub async fn run(sub: AgentCmd, json: bool) -> Result<()> {
    match sub {
        AgentCmd::Spawn {
            name,
            goal,
            capabilities,
        } => {
            let dispatcher = connect_config(None, None).await?;
            let pid = spawn_agent(
                &dispatcher,
                &name,
                &goal,
                &capabilities.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            )
            .await?;
            emit(
                json,
                |pid: &u64| format!("Agent spawned with PID {}", pid),
                pid,
            );
        }

        AgentCmd::List => {
            let dispatcher = connect_config(None, None).await?;
            let agents = list_agents(&dispatcher).await?;
            emit(
                json,
                |agents: &Vec<serde_json::Value>| format!("Agents: {:?}", agents),
                agents,
            );
        }

        AgentCmd::Kill { pid } => {
            let dispatcher = connect_config(None, None).await?;
            kill_agent(&dispatcher, pid).await?;
            emit(json, |_: &()| format!("Killed agent {}", pid), ());
        }

        AgentCmd::Catalog { username } => {
            let dispatcher = connect_config(None, None).await?;
            // Empty string signals the gateway to inject the caller's identity.
            let user = username.as_deref().unwrap_or("");
            let agents = list_installed(&dispatcher, user).await?;
            emit(json, format_catalog, agents);
        }

        AgentCmd::History {
            agent,
            username,
            live,
        } => {
            let dispatcher = connect_config(None, None).await?;
            let user = username.as_deref().unwrap_or("");
            let records = if live {
                list_invocations_live(&dispatcher, user, agent.as_deref()).await?
            } else {
                list_invocations(&dispatcher, user, agent.as_deref()).await?
            };
            emit(json, format_history, records);
        }

        AgentCmd::Show { invocation_id } => {
            let dispatcher = connect_config(None, None).await?;
            match get_invocation(&dispatcher, &invocation_id).await? {
                Some(inv) => emit(json, format_invocation, inv),
                None => {
                    eprintln!("Invocation '{}' not found.", invocation_id);
                    std::process::exit(1);
                }
            }
        }

        AgentCmd::Snapshot { invocation_id } => {
            let dispatcher = connect_config(None, None).await?;
            let result = snapshot_invocation(&dispatcher, &invocation_id).await?;
            emit(
                json,
                |r: &serde_json::Value| {
                    format!(
                        "Snapshot saved for invocation '{}' (tokens: {})",
                        invocation_id,
                        r["record"]["tokensConsumed"].as_u64().unwrap_or(0),
                    )
                },
                result,
            );
        }

        AgentCmd::Install {
            source,
            scope,
            version,
            checksum,
            no_verify,
            session,
            dry_run,
        } => {
            let dispatcher = connect_config(None, None).await?;

            if dry_run {
                let resolved = PackageSource::resolve(&source, version.as_deref()).await?;
                println!("Resolved source: {:?}", resolved);
                return Ok(());
            }

            let source = if source.starts_with("file://") {
                source.clone()
            } else if std::path::Path::new(&source).exists() {
                let abs =
                    std::fs::canonicalize(&source).context("failed to resolve absolute path")?;
                format!("file://{}", abs.display())
            } else {
                source
            };

            let body = serde_json::json!({
                "source":     source,
                "scope":      scope,
                "version":    version.as_deref().unwrap_or("latest"),
                "checksum":   checksum,
                "no_verify":  no_verify,
                "session_id": session,
            });

            let cmd = AtpCmd_::new("proc", "package/install-agent", &dispatcher.token, body);
            let reply = dispatcher
                .call(&cmd)
                .await
                .context("install-agent failed")?;

            if !reply.ok {
                let msg = reply
                    .message
                    .unwrap_or_else(|| "install-agent failed".into());
                anyhow::bail!("{}", msg);
            }

            println!(
                "Installed agent '{}' v{}",
                reply
                    .body
                    .as_ref()
                    .and_then(|b| b.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("?"),
                reply
                    .body
                    .as_ref()
                    .and_then(|b| b.get("version"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
            );
        }

        AgentCmd::Uninstall { name, scope } => {
            let dispatcher = connect_config(None, None).await?;

            let body = serde_json::json!({
                "name": name,
                "scope": scope,
            });

            let cmd = AtpCmd_::new("proc", "package/uninstall-agent", &dispatcher.token, body);
            let reply = dispatcher
                .call(&cmd)
                .await
                .context("uninstall-agent failed")?;

            if !reply.ok {
                let msg = reply
                    .message
                    .unwrap_or_else(|| "uninstall-agent failed".into());
                anyhow::bail!("{}", msg);
            }

            println!("Uninstalled agent '{}'", name);
        }
    }

    Ok(())
}
