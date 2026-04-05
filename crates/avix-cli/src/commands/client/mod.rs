pub mod agent;
pub mod hil;
pub mod secret;
pub mod service;
pub mod session;

pub use agent::AgentCmd;
pub use hil::HilCmd;
pub use secret::SecretCmd;
pub use service::ServiceCmd;
pub use session::SessionCmd;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

use avix_client_core::persistence;

use crate::util::{connect_config, emit, run_atp_shell};

#[derive(Subcommand)]
pub enum AtpCmd {
    /// Interactive ATP shell (REPL)
    Shell {
        /// ATP server URL
        #[arg(long, default_value = "ws://localhost:9142/atp")]
        server: String,
        /// Authentication token (prompts if omitted)
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ClientCmd {
    /// Test connectivity to the Avix server (reads config.yaml)
    Connect {
        /// Custom client config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Launch the TUI dashboard
    Tui {
        /// Enable structured trace output (comma-separated: notifications or all)
        #[arg(long)]
        trace: Option<String>,
        /// Custom client config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// ATP protocol commands
    Atp {
        #[command(subcommand)]
        sub: AtpCmd,
    },
    /// Manage agents on the running server
    Agent {
        #[command(subcommand)]
        sub: AgentCmd,
    },
    /// Manage human-in-the-loop requests
    Hil {
        #[command(subcommand)]
        sub: HilCmd,
    },
    /// Tail logs from the server
    Logs {
        /// Follow logs continuously
        #[arg(long)]
        follow: bool,
        /// Custom client config file
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Manage installed services
    Service {
        #[command(subcommand)]
        sub: ServiceCmd,
    },
    /// Manage runtime secrets
    Secret {
        #[command(subcommand)]
        sub: SecretCmd,
    },
    /// Manage agent sessions
    Session {
        #[command(subcommand)]
        sub: SessionCmd,
    },
}

pub async fn run(sub: ClientCmd, json: bool) -> Result<()> {
    match sub {
        ClientCmd::Connect { config } => {
            connect_config(config, None).await?;
            emit(json, |_: &()| "Connected to server".to_string(), ());
        }

        ClientCmd::Tui { trace, config: _ } => {
            let _tracer = trace.as_deref().map(|t| {
                let flags = avix_client_core::trace::ClientTraceFlags::from_csv(t);
                let log_dir = persistence::app_data_dir().join("logs");
                avix_client_core::trace::ClientTracer::new(flags, log_dir)
            });
            crate::tui::app::run(json).await?;
        }

        ClientCmd::Atp { sub } => match sub {
            AtpCmd::Shell { server, token } => {
                run_atp_shell(server, token).await?;
            }
        },

        ClientCmd::Agent { sub } => agent::run(sub, json).await?,
        ClientCmd::Hil { sub } => hil::run(sub, json).await?,
        ClientCmd::Session { sub } => session::run(sub, json).await?,
        ClientCmd::Service { sub } => service::run(sub, json).await?,
        ClientCmd::Secret { sub } => secret::run(sub, json).await?,

        ClientCmd::Logs { follow: _, config } => {
            let _config = config;
            emit(json, |_: &()| "Logs output".to_string(), ());
        }
    }

    Ok(())
}
