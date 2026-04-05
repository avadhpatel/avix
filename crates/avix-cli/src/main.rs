mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt};

use avix_client_core::atp::types::Cmd as AtpCmd_;
use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::commands::spawn_agent::spawn_agent;
use avix_client_core::commands::{
    get_invocation, kill_agent, list_agents, list_installed, list_invocations,
    list_invocations_live, resolve_hil, snapshot_invocation,
};
use avix_client_core::config::ClientConfig;
use avix_client_core::persistence;

use avix_core::service::package_source::PackageSource;
use avix_core::secrets::SecretStore;
use avix_core::service::{ServiceManager, ServiceStatus};

use avix_core::bootstrap::Runtime;
use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
use avix_core::cli::config_reload::{run_config_reload, ReloadParams};
use avix_core::cli::resolve::{run_resolve, ResolveParams};
use avix_core::config::{LlmConfig, ProviderAuth, ProviderConfig};
use avix_core::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
use avix_core::executor::spawn::SpawnParams;
use avix_core::llm_client::LlmClient;
use avix_core::llm_svc::adapter::xai::XaiAdapter;
use avix_core::llm_svc::autoagents_client::AutoAgentsChatClient;
use avix_core::llm_svc::DirectHttpLlmClient;
use avix_core::types::Modality;
// TODO: in daemon mode use IpcLlmClient to call a running llm.svc
use avix_core::types::token::CapabilityToken;
use avix_core::types::Pid;
#[allow(unused_imports)]
use avix_core::IpcLlmClient;

#[derive(Parser)]
#[command(name = "avix", about = "Avix agent OS", version)]
struct Cli {
    /// Output in JSON format
    #[arg(long)]
    json: bool,

    /// Log level
    #[arg(long = "log", default_value_t = LevelFilter::WARN, global = true)]
    log: LevelFilter,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Server-side: initialize, configure, and run the Avix runtime
    Server {
        #[command(subcommand)]
        sub: ServerCmd,
    },
    /// Client-side: connect to and interact with a running Avix server
    Client {
        #[command(subcommand)]
        sub: ClientCmd,
    },
    /// Build, validate, and scaffold Avix packages (offline — no server required)
    Package {
        #[command(subcommand)]
        sub: PackageCmd,
    },
}

// ── Service commands ──────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ServiceCmd {
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

// ── Secret commands ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum SecretCmd {
    /// Store a secret on disk (admin operation — requires AVIX_ROOT filesystem access)
    Set {
        /// Secret name
        name: String,
        /// Secret value
        value: String,
        /// Store for a service (mutually exclusive with --for-user)
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// Store for a user (mutually exclusive with --for-service)
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: std::path::PathBuf,
    },
    /// List secret names for an owner
    List {
        /// List secrets for a service
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// List secrets for a user
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: std::path::PathBuf,
    },
    /// Delete a secret from disk
    Delete {
        /// Secret name
        name: String,
        /// Delete from service owner
        #[arg(long = "for-service", conflicts_with = "for_user")]
        for_service: Option<String>,
        /// Delete from user owner
        #[arg(long = "for-user", conflicts_with = "for_service")]
        for_user: Option<String>,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: std::path::PathBuf,
    },
}

// ── Server commands ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ServerCmd {
    /// Start the Avix server daemon
    Start {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// ATP listen port
        #[arg(long, default_value = "9142")]
        port: u16,
        /// Enable test mode: mock IPC layer with seeded procs and periodic events
        #[arg(long)]
        test_mode: bool,
        /// Kernel IPC socket path (default: <root>/run/avix/kernel.sock)
        #[arg(long)]
        kernel_sock: Option<PathBuf>,
        /// Enable structured trace output (comma-separated: atp,agent,notifications or all)
        #[arg(long)]
        trace: Option<String>,
    },
    /// Server configuration management
    Config {
        #[command(subcommand)]
        sub: ServerConfigCmd,
    },
    /// Run an agent directly against the runtime (requires provider API key env var)
    Run {
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
        /// Goal for the agent
        #[arg(long, short)]
        goal: String,
        /// Agent name
        #[arg(long, default_value = "hello-agent")]
        name: String,
        /// Override the model (uses defaultProviders.text from etc/llm.yaml if omitted)
        #[arg(long)]
        model: Option<String>,
    },
    /// Resolve agent parameters for a user without spawning
    Resolve {
        /// Parameter kind to resolve (currently always `agent-manifest`)
        kind: String,
        /// User to resolve parameters for
        #[arg(long)]
        user: String,
        /// Agent manifest name (optional)
        #[arg(long)]
        agent: Option<String>,
        /// Include full annotation/provenance block in output
        #[arg(long)]
        explain: bool,
        /// Show effective limits only (no defaults merging)
        #[arg(long)]
        limits_only: bool,
        /// Simulate adding this crew membership for the resolution
        #[arg(long, name = "crew")]
        extra_crew: Option<String>,
        /// Print what would happen without writing any file
        #[arg(long)]
        dry_run: bool,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
}

#[derive(Subcommand)]
enum ServerConfigCmd {
    /// Create auth.conf and print the generated API key
    Init {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// Admin user name
        #[arg(long, default_value = "admin")]
        user: String,
        /// User role
        #[arg(long, default_value = "admin")]
        role: String,
    },
    /// Validate and apply hot-reloadable kernel config changes
    Reload {
        /// Only validate — do not write reload-pending marker
        #[arg(long)]
        check: bool,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
}

// ── Client commands ───────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum ClientCmd {
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

#[derive(Subcommand)]
enum AtpCmd {
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
enum AgentCmd {
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

#[derive(Subcommand)]
enum SessionCmd {
    /// Create a new session
    Create {
        /// Session title
        #[arg(long)]
        title: String,
        /// Session goal
        #[arg(long)]
        goal: String,
        /// Username (defaults to current user)
        #[arg(long)]
        username: Option<String>,
    },
    /// List sessions for a user
    List {
        /// Username to query (defaults to current user)
        #[arg(long)]
        username: Option<String>,
        /// Filter by status (idle, running, completed, failed)
        #[arg(long)]
        status: Option<String>,
    },
    /// Show session details
    Show {
        /// Session ID
        session_id: String,
    },
    /// Resume an idle session (spawn new invocation)
    Resume {
        /// Session ID
        session_id: String,
        /// Input to resume with
        #[arg(long)]
        input: Option<String>,
    },
}

#[derive(Subcommand)]
enum HilCmd {
    /// Approve a HIL request
    Approve {
        /// PID of the agent
        pid: u64,
        /// HIL request ID
        hil_id: String,
        /// Approval token
        #[arg(long)]
        token: String,
        /// Optional note
        #[arg(long)]
        note: Option<String>,
    },
    /// Deny a HIL request
    Deny {
        /// PID of the agent
        pid: u64,
        /// HIL request ID
        hil_id: String,
        /// Approval token
        #[arg(long)]
        token: String,
        /// Optional note
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
enum PackageCmd {
    /// Validate a package directory without building
    Validate {
        /// Path to the agent pack or service directory
        path: PathBuf,
    },
    /// Build a .tar.xz archive from a package directory
    Build {
        /// Path to the agent pack or service directory
        path: PathBuf,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// Version string (e.g. v0.1.0)
        #[arg(long)]
        version: String,
        /// Skip pre-build validation
        #[arg(long)]
        skip_validation: bool,
    },
    /// Scaffold a new agent pack or service directory
    New {
        /// Package name
        name: String,
        /// Package type: agent or service
        #[arg(long = "type", value_parser = ["agent", "service"])]
        pkg_type: String,
        /// Initial version (default: 0.1.0)
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,
    },
    /// Manage trusted third-party signing keys
    Trust {
        #[command(subcommand)]
        sub: TrustCmd,
    },
}

#[derive(Subcommand)]
enum TrustCmd {
    /// Add a trusted signing key
    Add {
        /// Path to a local .asc key file, or https:// URL to fetch it from
        key: String,
        /// Human-readable label for this key (e.g. "AcmeCorp")
        #[arg(long)]
        name: String,
        /// Restrict this key to specific source patterns (e.g. "github:acmecorp/*")
        /// May be specified multiple times. Omit to trust for all sources.
        #[arg(long = "allow-source")]
        allow_sources: Vec<String>,
    },
    /// List all trusted keys
    List,
    /// Remove a trusted key by fingerprint
    Remove {
        fingerprint: String,
    },
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Emit output in JSON or human-readable format
fn emit<T: serde::Serialize>(json_mode: bool, human_fn: impl FnOnce(&T) -> String, value: T) {
    if json_mode {
        println!("{}", serde_json::to_string(&value).unwrap());
    } else {
        println!("{}", human_fn(&value));
    }
}

fn format_catalog(agents: &Vec<serde_json::Value>) -> String {
    if agents.is_empty() {
        return "No agents installed.".to_string();
    }
    let mut out = format!(
        "{:<24} {:<10} {:<8} {}\n",
        "NAME", "VERSION", "SCOPE", "DESCRIPTION"
    );
    out.push_str(&"-".repeat(72));
    out.push('\n');
    for a in agents {
        let name = a["name"].as_str().unwrap_or("?");
        let version = a["version"].as_str().unwrap_or("?");
        let scope = a["scope"].as_str().unwrap_or("?");
        let desc = a["description"].as_str().unwrap_or("");
        out.push_str(&format!(
            "{:<24} {:<10} {:<8} {}\n",
            name, version, scope, desc
        ));
    }
    out
}

fn format_history(records: &Vec<serde_json::Value>) -> String {
    if records.is_empty() {
        return "No invocation history.".to_string();
    }
    let mut out = format!(
        "{:<12} {:<20} {:<12} {:<26} {}\n",
        "ID", "AGENT", "STATUS", "SPAWNED", "TOKENS"
    );
    out.push_str(&"-".repeat(80));
    out.push('\n');
    for r in records {
        let id = r["id"].as_str().unwrap_or("?");
        let short_id = if id.len() > 8 { &id[..8] } else { id };
        let agent = r["agentName"].as_str().unwrap_or("?");
        let status = r["status"].as_str().unwrap_or("?");
        let spawned = r["spawnedAt"].as_str().unwrap_or("?");
        let tokens = r["tokensConsumed"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "{:<12} {:<20} {:<12} {:<26} {}\n",
            short_id, agent, status, spawned, tokens
        ));
    }
    out
}

fn format_invocation(inv: &serde_json::Value) -> String {
    let mut out = String::new();
    out.push_str(&format!("ID:      {}\n", inv["id"].as_str().unwrap_or("?")));
    out.push_str(&format!(
        "Agent:   {}\n",
        inv["agentName"].as_str().unwrap_or("?")
    ));
    out.push_str(&format!(
        "Status:  {}\n",
        inv["status"].as_str().unwrap_or("?")
    ));
    out.push_str(&format!(
        "Goal:    {}\n",
        inv["goal"].as_str().unwrap_or("")
    ));
    out.push_str(&format!(
        "Spawned: {}\n",
        inv["spawnedAt"].as_str().unwrap_or("?")
    ));
    if let Some(ended) = inv["endedAt"].as_str() {
        out.push_str(&format!("Ended:   {}\n", ended));
    }
    out.push_str(&format!(
        "Tokens:  {}\n",
        inv["tokensConsumed"].as_u64().unwrap_or(0)
    ));
    out.push('\n');
    if let Some(messages) = inv["conversation"].as_array() {
        out.push_str("--- Conversation ---\n");
        for msg in messages {
            let role = msg["role"].as_str().unwrap_or("?");
            let content = msg["content"].as_str().unwrap_or("");
            out.push_str(&format!("[{}] {}\n", role, content));
        }
    }
    out
}

fn log_filename(cmd: &Cmd) -> &str {
    match cmd {
        Cmd::Server { sub } => match sub {
            ServerCmd::Start { .. } => "server",
            ServerCmd::Run { .. } => "run",
            _ => "server",
        },
        Cmd::Client { sub } => match sub {
            ClientCmd::Tui { .. } => "tui",
            ClientCmd::Hil { .. } => "hil",
            ClientCmd::Logs { .. } => "logs",
            ClientCmd::Agent { .. } => "agent",
            ClientCmd::Service { .. } => "service",
            ClientCmd::Secret { .. } => "secret",
            ClientCmd::Session { .. } => "session",
            _ => "client",
        },
        Cmd::Package { .. } => "package",
    }
}

/// Connect to the ATP server using config.yaml and return a dispatcher.
async fn connect_config(
    config: Option<PathBuf>,
    server_url: Option<String>,
) -> Result<Dispatcher, anyhow::Error> {
    let mut config = ClientConfig::load_from(config).unwrap_or_else(|_| ClientConfig::default());
    if let Some(url) = server_url {
        config.server_url = url;
    }
    let client = AtpClient::connect(config).await?;
    Ok(Dispatcher::new(client))
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let log_level = cli.log;
    let log_dir = persistence::app_data_dir().join("logs");
    let log_filename = log_filename(&cli.command);
    let appender = RollingFileAppender::new(Rotation::DAILY, log_dir.clone(), log_filename);
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(appender)
                .with_target(false)
                .with_thread_ids(true)
                .with_file(true)
                .with_line_number(true)
                .json(),
        )
        .with(log_level);
    tracing::subscriber::set_global_default(subscriber)?;
    tracing::info!(
        "log_dir={} level={:?} filename={}",
        log_dir.display(),
        cli.log,
        log_filename
    );

    match cli.command {
        // ── Server commands ───────────────────────────────────────────────────
        Cmd::Server { sub } => match sub {
            ServerCmd::Start {
                root,
                port,
                test_mode,
                kernel_sock,
                trace,
            } => {
                let root = expand_home(root);
                let kernel_sock = kernel_sock.unwrap_or_else(|| root.join("run/avix/kernel.sock"));
                std::env::set_var("AVIX_KERNEL_SOCK", kernel_sock);
                let trace_flags = trace
                    .as_deref()
                    .map(avix_core::trace::TraceFlags::from_csv)
                    .unwrap_or_default();
                let runtime = Runtime::bootstrap_with_root(&root)
                    .await?
                    .with_trace_flags(trace_flags);
                runtime.start_daemon(port, test_mode).await?;
            }

            ServerCmd::Config { sub } => match sub {
                ServerConfigCmd::Init { root, user, role } => {
                    let root = expand_home(root);
                    let result = run_config_init(ConfigInitParams {
                        root: root.clone(),
                        identity_name: user,
                        credential_type: "api_key".into(),
                        role,
                        master_key_source: "env".into(),
                        mode: "cli".into(),
                    })?;
                    println!("Avix runtime initialised at: {}", root.display());
                    println!("API key (Avix): {}", result.api_key);
                    println!();
                    println!("Next step:");
                    println!(
                        "  <PROVIDER>_API_KEY=<key> avix server start --root {} --port 9142",
                        root.display()
                    );
                }

                ServerConfigCmd::Reload { check, root } => {
                    let root = expand_home(root);
                    let result = run_config_reload(ReloadParams {
                        root,
                        check_only: check,
                    })
                    .await?;
                    if result.restart_required.is_empty() {
                        println!(
                            "Config valid — hot-reloadable sections: {}",
                            result.reloaded_sections.join(", ")
                        );
                        if check {
                            println!("(--check mode: no reload-pending marker written)");
                        }
                    } else {
                        eprintln!(
                            "WARNING: sections requiring restart: {}",
                            result.restart_required.join(", ")
                        );
                        if !result.reloaded_sections.is_empty() {
                            println!(
                                "Hot-reloadable sections: {}",
                                result.reloaded_sections.join(", ")
                            );
                        }
                        std::process::exit(1);
                    }
                }
            },

            ServerCmd::Run {
                root,
                goal,
                name,
                model,
            } => {
                let root = expand_home(root);

                let llm_config = load_llm_config(&root)?;
                let provider_cfg = llm_config
                    .default_provider_for(Modality::Text)
                    .ok_or_else(|| anyhow::anyhow!("no default text provider in etc/llm.yaml"))?;

                let resolved_model = model
                    .clone()
                    .or_else(|| default_text_model(provider_cfg))
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no text model found for provider '{}' in etc/llm.yaml",
                            provider_cfg.name
                        )
                    })?;

                let llm_client: Box<dyn LlmClient> =
                    build_llm_client(provider_cfg, &resolved_model)?;

                let runtime = Runtime::bootstrap_with_root(&root).await?;
                println!(
                    "Runtime booted — {} phases complete",
                    runtime.boot_log().len()
                );

                let token = CapabilityToken::test_token(&[
                    "cap/request-tool",
                    "cap/escalate",
                    "cap/list",
                    "job/watch",
                ]);
                let params = SpawnParams {
                    pid: Pid::new(100),
                    agent_name: name.clone(),
                    goal: goal.clone(),
                    spawned_by: "cli".into(),
                    session_id: uuid::Uuid::new_v4().to_string(),
                    token,
                    system_prompt: None,
                    selected_model: resolved_model.clone(),
                    denied_tools: vec![],
                    context_limit: 0,
                    runtime_dir: runtime.runtime_dir().to_path_buf(),
                    invocation_id: uuid::Uuid::new_v4().to_string(),
                };
                let registry = Arc::new(MockToolRegistry::new());
                let mut executor = RuntimeExecutor::spawn_with_registry(params, registry).await?;

                println!("Agent '{}' spawned (PID 100)", name);
                println!("Goal: {}", goal);
                println!();
                println!("--- Agent output ---");
                let result = executor.run_with_client(&goal, llm_client.as_ref()).await?;
                println!("{}", result.text);
                println!("--- Done ---");
            }

            ServerCmd::Resolve {
                kind,
                user,
                agent,
                explain,
                limits_only,
                extra_crew,
                dry_run,
                root,
            } => {
                let root = expand_home(root);
                let result = run_resolve(ResolveParams {
                    root,
                    kind,
                    username: user,
                    agent_name: agent,
                    explain,
                    limits_only,
                    extra_crew,
                    dry_run,
                })
                .await?;
                println!("{}", result.output);
            }
        },

        // ── Package commands (top-level, offline) ─────────────────────────────
        Cmd::Package { sub } => {
            use avix_core::packaging::{
                PackageBuilder, PackageScaffold, PackageValidator,
                BuildRequest, ScaffoldRequest,
            };

            match sub {
                PackageCmd::Validate { path } => {
                    match PackageValidator::validate(&path) {
                        Ok(pkg_type) => {
                            println!("✓ Valid {:?} package", pkg_type);
                        }
                        Err(errors) => {
                            eprintln!("Validation failed ({} error(s)):", errors.len());
                            for e in &errors {
                                eprintln!("  {}: {}", e.path, e.message);
                            }
                            std::process::exit(1);
                        }
                    }
                }
                PackageCmd::Build { path, output, version, skip_validation } => {
                    let output_dir = output.unwrap_or_else(|| std::env::current_dir().unwrap());
                    let req = BuildRequest { source_dir: path, output_dir, version, skip_validation };
                    let result = PackageBuilder::build(req)
                        .context("package build failed")?;
                    println!("Built: {}", result.archive_path.display());
                    println!("Checksum: {}", result.checksum_entry.trim());
                }
                PackageCmd::New { name, pkg_type, version, output } => {
                    let pkg_type = if pkg_type == "agent" {
                        avix_core::packaging::PackageType::Agent
                    } else {
                        avix_core::packaging::PackageType::Service
                    };
                    let dir = PackageScaffold::create(ScaffoldRequest { name: name.clone(), pkg_type, version, output_dir: output })
                        .context("scaffold failed")?;
                    println!("Created: {}", dir.display());
                }
                PackageCmd::Trust { sub } => {
                    let root = expand_home(std::path::PathBuf::from(
                        std::env::var("AVIX_ROOT").unwrap_or_else(|_| ".".to_string())
                    ));
                    
                    use avix_core::packaging::TrustStore;

                    match sub {
                        TrustCmd::Add { key, name, allow_sources } => {
                            let key_asc = if key.starts_with("https://") || key.starts_with("http://") {
                                let resp = reqwest::get(&key)
                                    .await
                                    .context("fetch key from URL")?;
                                resp.text().await.context("read key response")?
                            } else {
                                std::fs::read_to_string(&key).context("read key file")?
                            };
                            let store = TrustStore::new(&root);
                            let trusted = store.add(&key_asc, &name, allow_sources)
                                .context("add trusted key")?;
                            println!("Trusted key added: {} ({})", trusted.label, trusted.fingerprint);
                        }
                        TrustCmd::List => {
                            let store = TrustStore::new(&root);
                            let keys = store.list().context("list trusted keys")?;
                            if keys.is_empty() {
                                println!("No third-party keys trusted (official Avix key always active).");
                                return Ok(());
                            }
                            for k in &keys {
                                println!("{} — {} (added {})",
                                    k.fingerprint,
                                    k.label,
                                    k.added_at.format("%Y-%m-%d"),
                                );
                                if k.allowed_sources.is_empty() {
                                    println!("  allowed sources: all");
                                } else {
                                    for s in &k.allowed_sources {
                                        println!("  allowed source: {}", s);
                                    }
                                }
                            }
                        }
                        TrustCmd::Remove { fingerprint } => {
                            let store = TrustStore::new(&root);
                            store.remove(&fingerprint).context("remove trusted key")?;
                            println!("Removed key: {}", fingerprint);
                        }
                    }
                }
            }
        },

        // ── Client commands ───────────────────────────────────────────────────
        Cmd::Client { sub } => match sub {
            ClientCmd::Connect { config } => {
                connect_config(config, None).await?;
                emit(cli.json, |_: &()| "Connected to server".to_string(), ());
            }

            ClientCmd::Tui { trace, config: _ } => {
                let _tracer = trace.as_deref().map(|t| {
                    let flags = avix_client_core::trace::ClientTraceFlags::from_csv(t);
                    let log_dir = persistence::app_data_dir().join("logs");
                    avix_client_core::trace::ClientTracer::new(flags, log_dir)
                });
                return tui::app::run(cli.json).await;
            }

            ClientCmd::Atp { sub } => match sub {
                AtpCmd::Shell { server, token } => {
                    run_atp_shell(server, token).await?;
                }
            },

            ClientCmd::Agent { sub } => match sub {
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
                        cli.json,
                        |pid: &u64| format!("Agent spawned with PID {}", pid),
                        pid,
                    );
                }
                AgentCmd::List => {
                    let dispatcher = connect_config(None, None).await?;
                    let agents = list_agents(&dispatcher).await?;
                    emit(
                        cli.json,
                        |agents: &Vec<serde_json::Value>| format!("Agents: {:?}", agents),
                        agents,
                    );
                }
                AgentCmd::Kill { pid } => {
                    let dispatcher = connect_config(None, None).await?;
                    kill_agent(&dispatcher, pid).await?;
                    emit(cli.json, |_: &()| format!("Killed agent {}", pid), ());
                }
                AgentCmd::Catalog { username } => {
                    let dispatcher = connect_config(None, None).await?;
                    let user = username.as_deref().unwrap_or("default");
                    let agents = list_installed(&dispatcher, user).await?;
                    emit(cli.json, format_catalog, agents);
                }
                AgentCmd::History {
                    agent,
                    username,
                    live,
                } => {
                    let dispatcher = connect_config(None, None).await?;
                    let user = username.as_deref().unwrap_or("default");
                    let records = if live {
                        list_invocations_live(&dispatcher, user, agent.as_deref()).await?
                    } else {
                        list_invocations(&dispatcher, user, agent.as_deref()).await?
                    };
                    emit(cli.json, format_history, records);
                }
                AgentCmd::Show { invocation_id } => {
                    let dispatcher = connect_config(None, None).await?;
                    match get_invocation(&dispatcher, &invocation_id).await? {
                        Some(inv) => emit(cli.json, format_invocation, inv),
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
                        cli.json,
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

                    // Resolve local paths to absolute paths so the server can access them
                    let source = if source.starts_with("file://") {
                        source.clone()
                    } else if std::path::Path::new(&source).exists() {
                        let abs = std::fs::canonicalize(&source)
                            .context("failed to resolve absolute path")?;
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
                    let reply = dispatcher.call(&cmd).await.context("install-agent failed")?;

                    if !reply.ok {
                        let msg = reply.message.unwrap_or_else(|| "install-agent failed".into());
                        anyhow::bail!("{}", msg);
                    }

                    println!(
                        "Installed agent '{}' v{}",
                        reply.body.as_ref().and_then(|b| b.get("name")).and_then(|n| n.as_str()).unwrap_or("?"),
                        reply.body.as_ref().and_then(|b| b.get("version")).and_then(|n| n.as_str()).unwrap_or("?")
                    );
                }
                AgentCmd::Uninstall { name, scope } => {
                    let dispatcher = connect_config(None, None).await?;

                    let body = serde_json::json!({
                        "name": name,
                        "scope": scope,
                    });

                    let cmd = AtpCmd_::new("proc", "package/uninstall-agent", &dispatcher.token, body);
                    let reply = dispatcher.call(&cmd).await.context("uninstall-agent failed")?;

                    if !reply.ok {
                        let msg = reply.message.unwrap_or_else(|| "uninstall-agent failed".into());
                        anyhow::bail!("{}", msg);
                    }

                    println!("Uninstalled agent '{}'", name);
                }
            },

            ClientCmd::Hil { sub } => match sub {
                HilCmd::Approve {
                    pid,
                    hil_id,
                    token,
                    note,
                } => {
                    let dispatcher = connect_config(None, None).await?;
                    resolve_hil(&dispatcher, pid, &hil_id, &token, true, note.as_deref()).await?;
                    emit(
                        cli.json,
                        |_: &()| format!("Approved HIL {} for PID {}", hil_id, pid),
                        (),
                    );
                }
                HilCmd::Deny {
                    pid,
                    hil_id,
                    token,
                    note,
                } => {
                    let dispatcher = connect_config(None, None).await?;
                    resolve_hil(&dispatcher, pid, &hil_id, &token, false, note.as_deref()).await?;
                    emit(
                        cli.json,
                        |_: &()| format!("Denied HIL {} for PID {}", hil_id, pid),
                        (),
                    );
                }
            },

            ClientCmd::Logs { follow: _, config } => {
                let _config = config;
                // For now, stub
                emit(cli.json, |_: &()| "Logs output".to_string(), ());
            }

            // ── Session commands ─────────────────────────────────────────────────────
            ClientCmd::Session { sub } => match sub {
                SessionCmd::Create {
                    title,
                    goal,
                    username,
                } => {
                    let dispatcher = connect_config(None, None).await?;
                    let username = username.as_deref().unwrap_or("default");
                    let reply = dispatcher
                        .call(&AtpCmd_::new(
                            "proc",
                            "session-create",
                            "",
                            serde_json::json!({
                                "username": username,
                                "title": title,
                                "goal": goal,
                            }),
                        ))
                        .await?;
                    if !reply.ok {
                        anyhow::bail!(reply
                            .message
                            .unwrap_or_else(|| "create session failed".into()));
                    }
                    let body = reply.body.unwrap_or(serde_json::json!({}));
                    emit(
                        cli.json,
                        |b: &&serde_json::Value| {
                            format!(
                                "Created session: {}",
                                b["session_id"].as_str().unwrap_or("unknown")
                            )
                        },
                        &body,
                    );
                }
                SessionCmd::List { username, status } => {
                    let dispatcher = connect_config(None, None).await?;
                    let username = username.as_deref().unwrap_or("default");
                    let reply = dispatcher
                        .call(&AtpCmd_::new(
                            "proc",
                            "session-list",
                            "",
                            serde_json::json!({ "username": username }),
                        ))
                        .await?;
                    if !reply.ok {
                        anyhow::bail!(reply
                            .message
                            .unwrap_or_else(|| "list sessions failed".into()));
                    }
                    let body = reply.body.unwrap_or(serde_json::json!([]));
                    emit(
                        cli.json,
                        |b: &&serde_json::Value| {
                            let sessions = b.as_array().map(|a| a.to_vec()).unwrap_or_default();
                            if sessions.is_empty() {
                                "No sessions found".to_string()
                            } else {
                                let filtered: Vec<_> = if let Some(ref s) = status {
                                    sessions
                                        .iter()
                                        .filter(|sess| sess["status"].as_str() == Some(s.as_str()))
                                        .collect()
                                } else {
                                    sessions.iter().collect()
                                };
                                if filtered.is_empty() {
                                    format!("No {} sessions found", status.as_ref().unwrap())
                                } else {
                                    let lines: Vec<String> = filtered
                                        .iter()
                                        .map(|s| {
                                            format!(
                                                "  {} [{}] - {}",
                                                s["id"].as_str().unwrap_or("?"),
                                                s["status"].as_str().unwrap_or("?"),
                                                s["title"].as_str().unwrap_or("")
                                            )
                                        })
                                        .collect();
                                    format!("Sessions:\n{}", lines.join("\n"))
                                }
                            }
                        },
                        &body,
                    );
                }
                SessionCmd::Show { session_id } => {
                    let dispatcher = connect_config(None, None).await?;
                    let reply = dispatcher
                        .call(&AtpCmd_::new(
                            "proc",
                            "session-get",
                            "",
                            serde_json::json!({ "id": session_id }),
                        ))
                        .await?;
                    if !reply.ok {
                        anyhow::bail!(reply.message.unwrap_or_else(|| "get session failed".into()));
                    }
                    let body = reply.body.unwrap_or(serde_json::json!({}));
                    emit(
                        cli.json,
                        |b: &&serde_json::Value| {
                            format!(
                            "Session: {}\n  Title: {}\n  Goal: {}\n  Status: {}\n  Origin: {}\n  Primary: {}\n  Participants: {}",
                            b["id"].as_str().unwrap_or("?"),
                            b["title"].as_str().unwrap_or(""),
                            b["goal"].as_str().unwrap_or(""),
                            b["status"].as_str().unwrap_or("?"),
                            b["origin_agent"].as_str().unwrap_or(""),
                            b["primary_agent"].as_str().unwrap_or(""),
                            b["participants"]
                                .as_array()
                                .map(|a| a.len())
                                .unwrap_or(0)
                        )
                        },
                        &body,
                    );
                }
                SessionCmd::Resume { session_id, input } => {
                    let dispatcher = connect_config(None, None).await?;
                    let reply = dispatcher
                        .call(&AtpCmd_::new(
                            "proc",
                            "session-resume",
                            "",
                            serde_json::json!({
                                "session_id": session_id,
                                "input": input,
                            }),
                        ))
                        .await?;
                    if !reply.ok {
                        anyhow::bail!(reply
                            .message
                            .unwrap_or_else(|| "resume session failed".into()));
                    }
                    let body = reply.body.unwrap_or(serde_json::json!({}));
                    emit(
                        cli.json,
                        |b: &&serde_json::Value| {
                            format!("Resumed session, PID: {}", b["pid"].as_u64().unwrap_or(0))
                        },
                        &body,
                    );
                }
            },

            // ── Service commands ─────────────────────────────────────────────────────
            ClientCmd::Service { sub } => match sub {
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

                    // Resolve local paths to absolute paths so the server can access them
                    let source = if source.starts_with("file://") {
                        source.clone()
                    } else if std::path::Path::new(&source).exists() {
                        let abs = std::fs::canonicalize(&source)
                            .context("failed to resolve absolute path")?;
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
                    let reply = dispatcher.call(&cmd).await.context("install-service failed")?;

                    if !reply.ok {
                        let msg = reply.message.unwrap_or_else(|| "install-service failed".into());
                        anyhow::bail!("{}", msg);
                    }

                    println!(
                        "Installed service '{}' v{}",
                        reply.body.as_ref().and_then(|b| b.get("name")).and_then(|n| n.as_str()).unwrap_or("?"),
                        reply.body.as_ref().and_then(|b| b.get("version")).and_then(|n| n.as_str()).unwrap_or("?")
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
                        cli.json,
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
                    let status: ServiceStatus = serde_yaml::from_str(&yaml)
                        .with_context(|| "failed to parse status.yaml")?;

                    emit(
                        cli.json,
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
                    emit(cli.json, |_: &()| format!("Started {name}"), ());
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
                    emit(cli.json, |_: &()| format!("Stopped {name}"), ());
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
                    emit(cli.json, |_: &()| format!("Restarted {name}"), ());
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
                        let status_path =
                            root.join("proc/services").join(&name).join("status.yaml");
                        if status_path.exists() {
                            anyhow::bail!(
                                "service '{name}' may be running — use --force to kill it first"
                            );
                        }
                    }

                    std::fs::remove_dir_all(&svc_dir)
                        .with_context(|| format!("failed to remove {}", svc_dir.display()))?;
                    emit(cli.json, |_: &()| format!("Uninstalled {name}"), ());
                }

                ServiceCmd::Logs { name, follow: _ } => {
                    emit(
                        cli.json,
                        |_: &()| format!("Logs for {name}: (not yet implemented)"),
                        (),
                    );
                }
            },

            // ── Secret commands ───────────────────────────────────────────────────
            ClientCmd::Secret { sub } => {
                let master_key: [u8; 32] = {
                    let raw =
                        std::env::var("AVIX_MASTER_KEY").context("AVIX_MASTER_KEY is not set")?;
                    let bytes = raw.as_bytes();
                    let mut key = [0u8; 32];
                    let len = bytes.len().min(32);
                    key[..len].copy_from_slice(&bytes[..len]);
                    key
                };

                match sub {
                    SecretCmd::Set {
                        name,
                        value,
                        for_service,
                        for_user,
                        root,
                    } => {
                        let root = expand_home(root);
                        let owner = owner_from(for_service, for_user)?;
                        let store = SecretStore::new(&root.join("secrets"), &master_key);
                        store
                            .set(&owner, &name, &value)
                            .context("failed to set secret")?;
                        emit(
                            cli.json,
                            |_: &()| format!("Secret '{name}' set for {owner}"),
                            (),
                        );
                    }

                    SecretCmd::List {
                        for_service,
                        for_user,
                        root,
                    } => {
                        let root = expand_home(root);
                        let owner = owner_from(for_service, for_user)?;
                        let store = SecretStore::new(&root.join("secrets"), &master_key);
                        let names = store.list(&owner);
                        emit(
                            cli.json,
                            |names: &Vec<String>| {
                                if names.is_empty() {
                                    format!("No secrets for {owner}")
                                } else {
                                    names.join("\n")
                                }
                            },
                            names,
                        );
                    }

                    SecretCmd::Delete {
                        name,
                        for_service,
                        for_user,
                        root,
                    } => {
                        let root = expand_home(root);
                        let owner = owner_from(for_service, for_user)?;
                        let store = SecretStore::new(&root.join("secrets"), &master_key);
                        store
                            .delete(&owner, &name)
                            .context("failed to delete secret")?;
                        emit(
                            cli.json,
                            |_: &()| format!("Secret '{name}' deleted for {owner}"),
                            (),
                        );
                    }
                }
            }
        },
    }

    Ok(())
}

// ── Secret helpers ────────────────────────────────────────────────────────────

fn owner_from(for_service: Option<String>, for_user: Option<String>) -> Result<String> {
    match (for_service, for_user) {
        (Some(svc), _) => Ok(format!("service:{svc}")),
        (_, Some(user)) => Ok(format!("user:{user}")),
        _ => anyhow::bail!("specify --for-service <name> or --for-user <name>"),
    }
}

// ── LLM helpers ───────────────────────────────────────────────────────────────

/// Load and parse `{root}/etc/llm.yaml`.
fn load_llm_config(root: &std::path::Path) -> Result<LlmConfig> {
    let path = root.join("etc/llm.yaml");
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    LlmConfig::from_str(&src).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Pick the default text model from a provider config.
/// Prefers the first `standard`-tier text model; falls back to the first text model.
fn default_text_model(provider: &ProviderConfig) -> Option<String> {
    let text_models: Vec<_> = provider
        .models
        .iter()
        .filter(|m| m.modality == Modality::Text)
        .collect();
    text_models
        .iter()
        .find(|m| m.tier == "standard")
        .or_else(|| text_models.first())
        .map(|m| m.id.clone())
}

/// Build an LLM client from a `ProviderConfig` read out of `llm.yaml`.
fn build_llm_client(provider: &ProviderConfig, model: &str) -> Result<Box<dyn LlmClient>> {
    let api_key = match &provider.auth {
        ProviderAuth::ApiKey { secret_name, .. } => {
            Some(std::env::var(secret_name).with_context(|| {
                format!(
                    "{secret_name} not set — set this env var with your {} API key",
                    provider.name
                )
            })?)
        }
        ProviderAuth::None => None,
        ProviderAuth::Oauth2 { .. } => {
            return Err(anyhow::anyhow!(
                "OAuth2 providers are not yet supported in CLI mode"
            ))
        }
    };

    match provider.name.as_str() {
        "anthropic" => {
            use autoagents::llm::backends::anthropic::Anthropic;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<Anthropic>::new()
                .api_key(api_key.unwrap_or_default())
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        "openai" => {
            use autoagents::llm::backends::openai::OpenAI;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<OpenAI>::new()
                .api_key(api_key.unwrap_or_default())
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        "xai" => {
            let auth = api_key.map(|k| ("Authorization".to_string(), format!("Bearer {k}")));
            Ok(Box::new(DirectHttpLlmClient::new(
                "https://api.x.ai",
                model,
                auth,
                Arc::new(XaiAdapter::new()),
            )))
        }
        "ollama" => {
            use autoagents::llm::backends::ollama::Ollama;
            use autoagents::llm::builder::LLMBuilder;
            let p = LLMBuilder::<Ollama>::new()
                .model(model)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        other => Err(anyhow::anyhow!(
            "unsupported provider '{}' — supported: anthropic, openai, xai, ollama",
            other
        )),
    }
}

// ── ATP shell ─────────────────────────────────────────────────────────────────

/// Run the interactive ATP shell REPL.
async fn run_atp_shell(server_url: String, token: Option<String>) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::io::{self, Write};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };

    println!("ATP Shell — connecting to {}", server_url);

    let credential = if let Some(t) = token {
        t
    } else {
        print!("Credential: ");
        io::stdout().flush()?;
        let mut credential = String::new();
        io::stdin().read_line(&mut credential)?;
        credential.trim().to_string()
    };

    let client = reqwest::Client::new();
    let login_url = server_url
        .replace("ws://", "http://")
        .replace("wss://", "https://")
        .replace("/atp", "/atp/auth/login");
    let resp = client
        .post(&login_url)
        .json(&serde_json::json!({"identity": "test", "credential": credential}))
        .send()
        .await?;
    let body: Value = resp.json().await?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Login failed: {:?}", body))?
        .to_string();

    println!("Logged in, connecting WS...");

    let mut request = server_url.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {}", token).parse()?);
    let (ws_stream, _) = connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();

    let sub_msg = serde_json::json!({"type": "subscribe", "events": ["*"]});
    write.send(Message::Text(sub_msg.to_string())).await?;

    println!("Connected. Type JSON-RPC commands, or 'help', 'quit'.");
    println!("Events will be printed as received.");

    let event_handle = tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<Value>(&text) {
                        if event.get("type").is_some() && event["type"] != "reply" {
                            println!("EVENT: {}", serde_json::to_string_pretty(&event).unwrap());
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(e) => eprintln!("WS error: {}", e),
                _ => {}
            }
        }
    });

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        print!("atp> ");
        stdout.flush()?;
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        let line = line.trim();

        match line {
            "" => continue,
            "quit" | "exit" => break,
            "help" => {
                println!("Commands:");
                println!("  <json>  - Send JSON-RPC request");
                println!("  help    - This help");
                println!("  quit    - Exit");
                continue;
            }
            _ => match serde_json::from_str::<Value>(line) {
                Ok(mut req) => {
                    static mut ID: u64 = 0;
                    unsafe { ID += 1 };
                    req["jsonrpc"] = "2.0".into();
                    req["id"] = unsafe { ID }.into();
                    write.send(Message::Text(req.to_string())).await?;
                    println!("Sent: {}", req);
                }
                Err(_) => {
                    eprintln!("Invalid JSON. Try: {{\"method\": \"proc.list\", \"params\": {{}}}}");
                }
            },
        }
    }

    write.send(Message::Close(None)).await?;
    event_handle.abort();
    Ok(())
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn expand_home(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn client_connect_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "connect"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Connect { .. }
            }
        ));
    }

    #[test]
    fn client_connect_with_config_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "connect",
            "--config",
            "/custom/config.yaml",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub: ClientCmd::Connect { config },
            } => assert_eq!(config, Some(PathBuf::from("/custom/config.yaml"))),
            _ => panic!("wrong variant"),
        }
    }

    // ── Service command tests ───────────────────────────────────────────────────

    #[test]
    fn service_install_parses_source_and_checksum() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--checksum",
            "sha256:abc123",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub:
                            ServiceCmd::Install {
                                source,
                                checksum,
                                no_verify,
                                ..
                            },
                    },
            } => {
                assert_eq!(source, "./pkg.tar.gz");
                assert_eq!(checksum.as_deref(), Some("sha256:abc123"));
                assert!(!no_verify);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_install_no_verify_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--no-verify",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { no_verify, .. },
                    },
            } => assert!(no_verify),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_install_parses_root() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "./pkg.tar.gz",
            "--root",
            "/custom/root",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { root, .. },
                    },
            } => assert_eq!(root.to_string_lossy(), "/custom/root"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_install_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "agent",
            "install",
            "github:owner/repo/agent",
            "--scope",
            "system",
            "--version",
            "v1.0.0",
            "--checksum",
            "sha256:abc123",
            "--no-verify",
            "--session",
            "abc-123",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Agent {
                        sub:
                            AgentCmd::Install {
                                source,
                                scope,
                                version,
                                checksum,
                                no_verify,
                                session,
                                ..
                            },
                    },
            } => {
                assert_eq!(source, "github:owner/repo/agent");
                assert_eq!(scope, "system");
                assert_eq!(version.as_deref(), Some("v1.0.0"));
                assert_eq!(checksum.as_deref(), Some("sha256:abc123"));
                assert!(no_verify);
                assert_eq!(session.as_deref(), Some("abc-123"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn agent_install_dry_run_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "agent",
            "install",
            "./agent.tar.xz",
            "--dry-run",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Agent {
                        sub: AgentCmd::Install { dry_run, .. },
                    },
            } => assert!(dry_run),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_list_subcommand_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::List { .. }
                }
            }
        ));
    }

    #[test]
    fn service_status_parses_name() {
        let cli =
            Cli::try_parse_from(["avix", "client", "service", "status", "github-svc"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Status { name, .. },
                    },
            } => assert_eq!(name, "github-svc"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_start_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "start", "my-svc"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Start { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_start_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "start",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Start { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_stop_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "stop", "my-svc"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Stop { .. }
                }
            }
        ));
    }

    #[test]
    fn service_stop_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "stop",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Stop { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_restart_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "service", "restart", "my-svc"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Restart { .. }
                }
            }
        ));
    }

    #[test]
    fn service_restart_parses_server_url() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "restart",
            "my-svc",
            "--server-url",
            "http://localhost:9999",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Restart { name, server_url },
                    },
            } => {
                assert_eq!(name, "my-svc");
                assert_eq!(server_url, Some("http://localhost:9999".to_string()));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_uninstall_force_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "uninstall",
            "github-svc",
            "--force",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Uninstall { name, force, .. },
                    },
            } => {
                assert_eq!(name, "github-svc");
                assert!(force);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_logs_follow_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "logs",
            "github-svc",
            "--follow",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Logs { name, follow },
                    },
            } => {
                assert_eq!(name, "github-svc");
                assert!(follow);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn service_status_json_flag() {
        let cli = Cli::try_parse_from([
            "avix",
            "--json",
            "client",
            "service",
            "status",
            "github-svc",
        ])
        .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Service {
                    sub: ServiceCmd::Status { .. }
                }
            }
        ));
    }

    #[test]
    fn service_install_no_checksum_defaults_none() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "service",
            "install",
            "https://example.com/x.tar.gz",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Service {
                        sub: ServiceCmd::Install { checksum, .. },
                    },
            } => assert!(checksum.is_none()),
            _ => panic!("wrong variant"),
        }
    }

    // ── Secret command tests ──────────────────────────────────────────────────

    #[test]
    fn secret_set_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "set",
            "github-app-key",
            "ghp_abc",
            "--for-service",
            "github-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Set {
                                name,
                                value,
                                for_service,
                                for_user,
                                ..
                            },
                    },
            } => {
                assert_eq!(name, "github-app-key");
                assert_eq!(value, "ghp_abc");
                assert_eq!(for_service.as_deref(), Some("github-svc"));
                assert!(for_user.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_set_for_user_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "set",
            "my-token",
            "tok-xyz",
            "--for-user",
            "alice",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Set {
                                name,
                                value,
                                for_service,
                                for_user,
                                ..
                            },
                    },
            } => {
                assert_eq!(name, "my-token");
                assert_eq!(value, "tok-xyz");
                assert!(for_service.is_none());
                assert_eq!(for_user.as_deref(), Some("alice"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_list_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "list",
            "--for-service",
            "github-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub: SecretCmd::List { for_service, .. },
                    },
            } => assert_eq!(for_service.as_deref(), Some("github-svc")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_list_for_user_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "secret", "list", "--for-user", "alice"])
            .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub: SecretCmd::List { for_user, .. },
                    },
            } => assert_eq!(for_user.as_deref(), Some("alice")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn secret_delete_for_service_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "secret",
            "delete",
            "my-key",
            "--for-service",
            "my-svc",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Secret {
                        sub:
                            SecretCmd::Delete {
                                name, for_service, ..
                            },
                    },
            } => {
                assert_eq!(name, "my-key");
                assert_eq!(for_service.as_deref(), Some("my-svc"));
            }
            _ => panic!("wrong variant"),
        }
    }

    // ── Session command tests ──────────────────────────────────────────────────

    #[test]
    fn session_create_parses() {
        let cli = Cli::try_parse_from([
            "avix",
            "client",
            "session",
            "create",
            "--title",
            "My Session",
            "--goal",
            "Do something",
        ])
        .unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Session {
                        sub: SessionCmd::Create { title, goal, .. },
                    },
            } => {
                assert_eq!(title, "My Session");
                assert_eq!(goal, "Do something");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn session_list_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "session", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Cmd::Client {
                sub: ClientCmd::Session {
                    sub: SessionCmd::List { .. }
                }
            }
        ));
    }

    #[test]
    fn session_show_parses() {
        let cli = Cli::try_parse_from(["avix", "client", "session", "show", "sess-123"]).unwrap();
        match cli.command {
            Cmd::Client {
                sub:
                    ClientCmd::Session {
                        sub: SessionCmd::Show { session_id },
                    },
            } => assert_eq!(session_id, "sess-123"),
            _ => panic!("wrong variant"),
        }
    }

    // ── Package command tests (top-level) ─────────────────────────────────────

    #[test]
    fn package_validate_parses() {
        let cli = Cli::try_parse_from(["avix", "package", "validate", "/path/to/pkg"]).unwrap();
        match cli.command {
            Cmd::Package {
                sub: PackageCmd::Validate { path },
            } => assert_eq!(path, PathBuf::from("/path/to/pkg")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_build_parses() {
        let cli = Cli::try_parse_from([
            "avix", "package", "build", "./agent-pack",
            "--output", "/out",
            "--version", "v1.0.0",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::Build {
                        path,
                        output,
                        version,
                        skip_validation: _,
                    },
            } => {
                assert_eq!(path, PathBuf::from("./agent-pack"));
                assert_eq!(output, Some(PathBuf::from("/out")));
                assert_eq!(version, "v1.0.0");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_new_parses() {
        let cli = Cli::try_parse_from([
            "avix", "package", "new", "my-agent",
            "--type", "agent",
            "--version", "0.2.0",
            "--output", "/tmp/packages",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::New {
                        name,
                        pkg_type,
                        version,
                        output,
                    },
            } => {
                assert_eq!(name, "my-agent");
                assert_eq!(pkg_type, "agent");
                assert_eq!(version, "0.2.0");
                assert_eq!(output, PathBuf::from("/tmp/packages"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn package_trust_add_parses() {
        let cli = Cli::try_parse_from([
            "avix", "package", "trust", "add",
            "--name", "Dev Key",
            "/keys/dev.asc",
            "--allow-source", "github.com/user/repo",
        ])
        .unwrap();
        match cli.command {
            Cmd::Package {
                sub:
                    PackageCmd::Trust {
                        sub: TrustCmd::Add { key, name, allow_sources },
                    },
            } => {
                assert_eq!(key, "/keys/dev.asc");
                assert_eq!(name, "Dev Key");
                assert_eq!(allow_sources, vec!["github.com/user/repo".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
    }
}
