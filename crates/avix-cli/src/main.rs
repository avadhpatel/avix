mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt};

use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::commands::spawn_agent::spawn_agent;
use avix_client_core::commands::{kill_agent, list_agents, resolve_hil};
use avix_client_core::config::ClientConfig;
use avix_client_core::persistence;

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
    #[arg(long = "log", default_value_t = LevelFilter::WARN)]
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
    Connect,
    /// Launch the TUI dashboard
    Tui,
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Emit output in JSON or human-readable format
fn emit<T: serde::Serialize>(json_mode: bool, human_fn: impl FnOnce(&T) -> String, value: T) {
    if json_mode {
        println!("{}", serde_json::to_string(&value).unwrap());
    } else {
        println!("{}", human_fn(&value));
    }
}

fn log_filename(cmd: &Cmd) -> &str {
    match cmd {
        Cmd::Server { sub } => match sub {
            ServerCmd::Start { .. } => "server",
            ServerCmd::Run { .. } => "run",
            _ => "server",
        },
        Cmd::Client { sub } => match sub {
            ClientCmd::Tui => "tui",
            ClientCmd::Hil { .. } => "hil",
            ClientCmd::Logs { .. } => "logs",
            ClientCmd::Agent { .. } => "agent",
            _ => "client",
        },
    }
}

/// Connect to the ATP server using config.yaml and return a dispatcher.
async fn connect_config() -> Result<Dispatcher, anyhow::Error> {
    let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
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
            } => {
                let root = expand_home(root);
                let kernel_sock = kernel_sock.unwrap_or_else(|| root.join("run/avix/kernel.sock"));
                std::env::set_var("AVIX_KERNEL_SOCK", kernel_sock);
                let runtime = Runtime::bootstrap_with_root(&root).await?;
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
                };
                let registry = Arc::new(MockToolRegistry::new());
                let mut executor =
                    RuntimeExecutor::spawn_with_registry(params, registry).await?;

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

        // ── Client commands ───────────────────────────────────────────────────
        Cmd::Client { sub } => match sub {
            ClientCmd::Connect => {
                connect_config().await?;
                emit(cli.json, |_: &()| "Connected to server".to_string(), ());
            }

            ClientCmd::Tui => {
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
                    let dispatcher = connect_config().await?;
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
                    let dispatcher = connect_config().await?;
                    let agents = list_agents(&dispatcher).await?;
                    emit(
                        cli.json,
                        |agents: &Vec<serde_json::Value>| format!("Agents: {:?}", agents),
                        agents,
                    );
                }
                AgentCmd::Kill { pid } => {
                    let dispatcher = connect_config().await?;
                    kill_agent(&dispatcher, pid).await?;
                    emit(cli.json, |_: &()| format!("Killed agent {}", pid), ());
                }
            },

            ClientCmd::Hil { sub } => match sub {
                HilCmd::Approve {
                    pid,
                    hil_id,
                    token,
                    note,
                } => {
                    let dispatcher = connect_config().await?;
                    resolve_hil(&dispatcher, pid, &hil_id, &token, true, note.as_deref())
                        .await?;
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
                    let dispatcher = connect_config().await?;
                    resolve_hil(&dispatcher, pid, &hil_id, &token, false, note.as_deref())
                        .await?;
                    emit(
                        cli.json,
                        |_: &()| format!("Denied HIL {} for PID {}", hil_id, pid),
                        (),
                    );
                }
            },

            ClientCmd::Logs { follow: _ } => {
                // For now, stub
                emit(cli.json, |_: &()| "Logs output".to_string(), ());
            }
        },
    }

    Ok(())
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
                    eprintln!(
                        "Invalid JSON. Try: {{\"method\": \"proc.list\", \"params\": {{}}}}"
                    );
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
