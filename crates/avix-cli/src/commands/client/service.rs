use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

use avix_client_core::atp::types::Cmd as AtpCmd_;
use avix_core::service::package_source::PackageSource;
use avix_core::service::{ServiceManager, ServiceStatus};

use crate::util::{connect_config, emit, expand_home};

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Install a service from a local path (file://), URL (https://), or GitHub source
    Install {
        /// Package source — local path, https:// URL, or github:owner/repo/name
        source: String,
        /// Install scope: `system` (default) or `user`
        #[arg(long, default_value = "system")]
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
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// List all installed services
    List {
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// Show full status of a service
    Status {
        /// Service name
        name: String,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
        /// ATP server URL to connect to
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Start a stopped/failed service
    Start {
        /// Service name
        name: String,
        /// ATP server URL to connect to
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Gracefully stop a running service
    Stop {
        /// Service name
        name: String,
        /// ATP server URL to connect to
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Stop then start a service
    Restart {
        /// Service name
        name: String,
        /// ATP server URL to connect to
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Remove a service from disk
    Uninstall {
        /// Service name
        name: String,
        /// Kill the service if running before uninstalling
        #[arg(long)]
        force: bool,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// Stream service output logs
    Logs {
        /// Service name
        name: String,
        /// Follow log output continuously
        #[arg(long)]
        follow: bool,
    },
}

pub async fn run(sub: ServiceCmd, json: bool) -> Result<()> {
    match sub {
        ServiceCmd::Install {
            source,
            scope,
            version,
            checksum,
            no_verify,
            session,
            dry_run,
            root: _,
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

            let cmd = AtpCmd_::new("proc", "package/install-service", &dispatcher.token, body);
            let reply = dispatcher
                .call(&cmd)
                .await
                .context("install-service failed")?;

            if !reply.ok {
                let msg = reply
                    .message
                    .unwrap_or_else(|| "install-service failed".into());
                anyhow::bail!("{}", msg);
            }

            println!(
                "Installed service '{}' v{}",
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

        ServiceCmd::List { root } => {
            let root = expand_home(root);
            let units = ServiceManager::discover_installed(&root)
                .context("failed to read installed services")?;

            #[derive(serde::Serialize)]
            struct Row {
                name: String,
                version: String,
                tool_count: usize,
            }
            let rows: Vec<Row> = units
                .into_iter()
                .map(|u| Row {
                    name: u.name,
                    version: u.version,
                    tool_count: u.tools.provides.len(),
                })
                .collect();

            emit(
                json,
                |rows: &Vec<Row>| {
                    if rows.is_empty() {
                        return "No services installed.".into();
                    }
                    let mut out = format!("{:<20} {:<10} {}\n", "NAME", "VERSION", "TOOLS");
                    for r in rows {
                        out.push_str(&format!(
                            "{:<20} {:<10} {}\n",
                            r.name, r.version, r.tool_count
                        ));
                    }
                    out
                },
                rows,
            );
        }

        ServiceCmd::Status {
            name,
            root,
            server_url: _,
        } => {
            let root = expand_home(root);
            let status_path = root.join("proc/services").join(&name).join("status.yaml");

            if !status_path.exists() {
                anyhow::bail!("no status file for service '{name}' — is it running?");
            }
            let yaml = std::fs::read_to_string(&status_path)
                .with_context(|| format!("cannot read {}", status_path.display()))?;
            let status: ServiceStatus =
                serde_yaml::from_str(&yaml).with_context(|| "failed to parse status.yaml")?;

            emit(
                json,
                |s: &ServiceStatus| {
                    format!(
                        "name:          {}\nversion:       {}\npid:           {}\nstate:         {:?}\nendpoint:      {}\ntools:         {}\nrestart_count: {}",
                        s.name,
                        s.version,
                        s.pid.as_u32(),
                        s.state,
                        s.endpoint.as_deref().unwrap_or("-"),
                        s.tools.join(", "),
                        s.restart_count,
                    )
                },
                status,
            );
        }

        ServiceCmd::Start { name, server_url } => {
            let dispatcher = connect_config(None, server_url).await?;
            let reply = dispatcher
                .call(&AtpCmd_::new(
                    "signal",
                    "send",
                    "",
                    serde_json::json!({ "name": name, "signal": "SIGSTART" }),
                ))
                .await?;
            if !reply.ok {
                anyhow::bail!(reply.message.unwrap_or_else(|| "start failed".into()));
            }
            emit(json, |_: &()| format!("Started {name}"), ());
        }

        ServiceCmd::Stop { name, server_url } => {
            let dispatcher = connect_config(None, server_url).await?;
            let reply = dispatcher
                .call(&AtpCmd_::new(
                    "signal",
                    "send",
                    "",
                    serde_json::json!({ "name": name, "signal": "SIGTERM" }),
                ))
                .await?;
            if !reply.ok {
                anyhow::bail!(reply.message.unwrap_or_else(|| "stop failed".into()));
            }
            emit(json, |_: &()| format!("Stopped {name}"), ());
        }

        ServiceCmd::Restart { name, server_url } => {
            let dispatcher = connect_config(None, server_url).await?;
            for (signal, label) in [("SIGSTOP", "stop"), ("SIGSTART", "start")] {
                let reply = dispatcher
                    .call(&AtpCmd_::new(
                        "signal",
                        "send",
                        "",
                        serde_json::json!({ "name": name, "signal": signal }),
                    ))
                    .await?;
                if !reply.ok {
                    anyhow::bail!(reply
                        .message
                        .unwrap_or_else(|| format!("restart ({label}) failed")));
                }
            }
            emit(json, |_: &()| format!("Restarted {name}"), ());
        }

        ServiceCmd::Uninstall { name, force, root } => {
            let root = expand_home(root);
            let svc_dir = root.join("services").join(&name);

            if !svc_dir.exists() {
                anyhow::bail!("service '{name}' is not installed");
            }

            if force {
                if let Ok(dispatcher) = connect_config(None, None).await {
                    let _ = dispatcher
                        .call(&AtpCmd_::new(
                            "signal",
                            "send",
                            "",
                            serde_json::json!({ "name": name, "signal": "SIGKILL" }),
                        ))
                        .await;
                }
            } else {
                let status_path = root.join("proc/services").join(&name).join("status.yaml");
                if status_path.exists() {
                    anyhow::bail!("service '{name}' may be running — use --force to kill it first");
                }
            }

            std::fs::remove_dir_all(&svc_dir)
                .with_context(|| format!("failed to remove {}", svc_dir.display()))?;
            emit(json, |_: &()| format!("Uninstalled {name}"), ());
        }

        ServiceCmd::Logs { name, follow: _ } => {
            emit(
                json,
                |_: &()| format!("Logs for {name}: (not yet implemented)"),
                (),
            );
        }
    }

    Ok(())
}
