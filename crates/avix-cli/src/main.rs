mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{filter::LevelFilter, layer::SubscriberExt};

use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::commands::{list_agents, resolve_hil, send_signal, spawn_agent};
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

    /// ATP server URL
    #[arg(long, default_value = "ws://localhost:9142/atp")]
    url: String,

    /// Authentication token
    #[arg(long, default_value = "token")]
    token: String,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialise a new Avix runtime root
    Config {
        #[command(subcommand)]
        sub: ConfigCmd,
    },
    /// Run the Avix server
    Server {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// ATP port (default 9142)
        #[arg(long, default_value = "9142")]
        port: u16,
        /// Enable test mode: mock IPC layer with seeded procs and periodic events
        #[arg(long)]
        test_mode: bool,
    },
    /// Run an agent (requires AVIX_MASTER_KEY + provider API key env var)
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
    /// Resolve agent parameters for a user (without spawning an agent)
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
    /// Connect to an Avix ATP server
    Connect,
    /// ATP protocol commands
    Atp {
        #[command(subcommand)]
        sub: AtpCmd,
    },
    /// Manage agents
    Agent {
        #[command(subcommand)]
        sub: AgentCmd,
    },
    /// Manage human-in-the-loop requests
    Hil {
        #[command(subcommand)]
        sub: HilCmd,
    },
    /// Launch TUI dashboard
    Tui,
    /// Tail logs from the server
    Logs {
        /// Follow logs
        #[arg(long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
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
        /// Only validate and classify sections — do not write reload-pending marker
        #[arg(long)]
        check: bool,
        /// Runtime root directory
        #[arg(long, default_value = "~/avix-data")]
        root: PathBuf,
    },
    /// Run the Avix server
    Server {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// ATP port (default 9142)
        #[arg(long, default_value = "9142")]
        port: u16,
        /// Enable test mode: mock IPC layer with seeded procs and periodic events
        #[arg(long)]
        test_mode: bool,
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
    /// Kill an agent
    Kill {
        /// PID of the agent
        pid: u64,
    },
    /// Run the Avix server
    Server {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// ATP port (default 9142)
        #[arg(long, default_value = "9142")]
        port: u16,
        /// Enable test mode: mock IPC layer with seeded procs and periodic events
        #[arg(long)]
        test_mode: bool,
    },
}

#[derive(Subcommand)]
enum HilCmd {
    /// Approve a HIL request
    Approve {
        /// PID of the agent
        pid: u64,
        /// HIL ID
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
        /// HIL ID
        hil_id: String,
        /// Approval token
        #[arg(long)]
        token: String,
        /// Optional note
        #[arg(long)]
        note: Option<String>,
    },
    /// Run the Avix server
    Server {
        /// Runtime root directory
        #[arg(long)]
        root: PathBuf,
        /// ATP port (default 9142)
        #[arg(long, default_value = "9142")]
        port: u16,
        /// Enable test mode: mock IPC layer with seeded procs and periodic events
        #[arg(long)]
        test_mode: bool,
    },
}

#[derive(Subcommand)]
enum AtpCmd {
    /// Interactive ATP shell (REPL)
    Shell {
        /// ATP server URL (default ws://localhost:9142/atp)
        #[arg(long, default_value = "ws://localhost:9142/atp")]
        server: String,
        /// Authentication token (if not provided, prompts for login)
        #[arg(long)]
        token: Option<String>,
    },
}

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
        Cmd::Server { .. } => "server",
        Cmd::Tui => "tui",
        Cmd::Agent { .. } => "agent",
        Cmd::Hil { .. } => "hil",
        Cmd::Logs { .. } => "logs",
        _ => "cli",
    }
}

/// Connect to ATP server using loaded config and return dispatcher
async fn connect_config() -> Result<Dispatcher, anyhow::Error> {
    let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
    let client = AtpClient::connect(config).await?;
    let dispatcher = Dispatcher::new(client);
    Ok(dispatcher)
}

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
    tracing::info!("log_dir={} level={:?} filename={}", log_dir.display(), cli.log, log_filename);

    match cli.command {
        Cmd::Config { sub } => match sub {
            ConfigCmd::Init { root, user, role } => {
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
                    "  AVIX_MASTER_KEY=<key> <PROVIDER>_API_KEY=<key> \\\n  avix run --root {} --provider <anthropic|openai|xai|ollama> --goal \\\"say hello world\\\"",
                    root.display()
                );
            }

            ConfigCmd::Reload { check, root } => {
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

            ConfigCmd::Server { root, port, test_mode } => {
                let root = expand_home(root);
                let runtime = Runtime::bootstrap_with_root(&root).await?;
                runtime.start_daemon(port, test_mode).await?;
            }
        },

        Cmd::Resolve {
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

        Cmd::Run {
            root,
            goal,
            name,
            model,
        } => {
            let root = expand_home(root);

            // Load provider config from etc/llm.yaml in the runtime root
            let llm_config = load_llm_config(&root)?;
            let provider_cfg = llm_config
                .default_provider_for(Modality::Text)
                .ok_or_else(|| anyhow::anyhow!("no default text provider in etc/llm.yaml"))?;

            // Resolve the model name before building the client so we can also
            // pass it to SpawnParams (RuntimeExecutor sends empty model string).
            let resolved_model = model
                .clone()
                .or_else(|| default_text_model(provider_cfg))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no text model found for provider '{}' in etc/llm.yaml",
                        provider_cfg.name
                    )
                })?;

            // Build the LLM client via AutoAgents — type-erased as Box<dyn LlmClient>
            let llm_client: Box<dyn LlmClient> = build_llm_client(provider_cfg, &resolved_model)?;

            // Bootstrap: checks auth.conf, reads+zeroes AVIX_MASTER_KEY
            let runtime = Runtime::bootstrap_with_root(&root).await?;
            println!(
                "Runtime booted — {} phases complete",
                runtime.boot_log().len()
            );

            // Spawn executor with minimal capability token
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

        Cmd::Server { root, port, test_mode } => {
            let root = expand_home(root);
            let runtime = Runtime::bootstrap_with_root(&root).await?;
            runtime.start_daemon(port, test_mode).await?;
        }

        Cmd::Connect => {
            connect_config().await?;
            emit(cli.json, |_: &()| "Connected to server".to_string(), ());
        }

        Cmd::Atp { sub } => match sub {
            AtpCmd::Shell { server, token } => {
                run_atp_shell(server, token).await?;
            }
        },

        Cmd::Agent { sub } => match sub {
            AgentCmd::Spawn {
                name,
                goal,
                capabilities,
            } => {
                let dispatcher = connect_config().await?;
                let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
                let pid = spawn_agent(
                    &dispatcher,
                    &config.credential,
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
                let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
                let agents = list_agents(&dispatcher, &config.credential).await?;
                emit(
                    cli.json,
                    |agents: &Vec<serde_json::Value>| format!("Agents: {:?}", agents),
                    agents,
                );
            }
            AgentCmd::Kill { pid } => {
                let dispatcher = connect_config().await?;
                let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
                send_signal(&dispatcher, &config.credential, pid, "SIGKILL", None).await?;
                emit(cli.json, |_: &()| format!("Killed agent {}", pid), ());
            }
            AgentCmd::Server { root, port, test_mode } => {
                let root = expand_home(root);
                let runtime = Runtime::bootstrap_with_root(&root).await?;
                runtime.start_daemon(port, test_mode).await?;
            }
        },

        Cmd::Hil { sub } => match sub {
            HilCmd::Approve {
                pid,
                hil_id,
                token,
                note,
            } => {
                let dispatcher = connect_config().await?;
                let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
                resolve_hil(
                    &dispatcher,
                    &config.credential,
                    pid,
                    &hil_id,
                    &token,
                    true,
                    note.as_deref(),
                )
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
                let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
                resolve_hil(
                    &dispatcher,
                    &config.credential,
                    pid,
                    &hil_id,
                    &token,
                    false,
                    note.as_deref(),
                )
                .await?;
                emit(
                    cli.json,
                    |_: &()| format!("Denied HIL {} for PID {}", hil_id, pid),
                    (),
                );
            }
            HilCmd::Server { root, port, test_mode } => {
                let root = expand_home(root);
                let runtime = Runtime::bootstrap_with_root(&root).await?;
                runtime.start_daemon(port, test_mode).await?;
            }
        },

        Cmd::Tui => {
            return tui::app::run(cli.json).await;
        }

        Cmd::Logs { follow: _ } => {
            // For now, stub
            emit(cli.json, |_: &()| "Logs output".to_string(), ());
        }
    }

    Ok(())
}

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
/// The API key is read from the env var named by `auth.secretName`.
/// `model` is the already-resolved model name (from `--model` or `llm.yaml`).
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
            // The autoagents XAI backend does not implement tool calling.
            // Use DirectHttpLlmClient with XaiAdapter — xAI's API is
            // OpenAI-compatible and supports function calling natively.
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

/// Run the interactive ATP shell REPL.
/// Connects to the given server, logs in if no token, subscribes to all events, then enters REPL.
/// Links: docs/dev_plans/ATP-WS-TESTS-PLAN.md#52
async fn run_atp_shell(server_url: String, token: Option<String>) -> Result<()> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message}};
    use serde_json::Value;
    use std::io::{self, Write};

    println!("ATP Shell — connecting to {}", server_url);

    // If token provided, use as credential; else prompt
    let credential = if let Some(t) = token {
        t
    } else {
        print!("Credential: ");
        io::stdout().flush()?;
        let mut credential = String::new();
        io::stdin().read_line(&mut credential)?;
        credential.trim().to_string()
    };

    // HTTP login
    let client = reqwest::Client::new();
    let login_url = server_url.replace("ws://", "http://").replace("wss://", "https://").replace("/atp", "/atp/auth/login");
    let resp = client
        .post(&login_url)
        .json(&serde_json::json!({"identity": "test", "credential": credential}))
        .send()
        .await?;
    let body: Value = resp.json().await?;
    let token = body["token"].as_str().ok_or_else(|| anyhow::anyhow!("Login failed: {:?}", body))?.to_string();

    println!("Logged in, connecting WS...");

    // Connect WS
    let mut request = server_url.into_client_request()?;
    request.headers_mut().insert("Authorization", format!("Bearer {}", token).parse()?);
    let (ws_stream, _) = connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();

    // Subscribe to all events
    let sub_msg = serde_json::json!({"type": "subscribe", "events": ["*"]});
    write.send(Message::Text(sub_msg.to_string())).await?;

    println!("Connected. Type JSON-RPC commands, or 'help', 'quit'.");
    println!("Events will be printed as received.");

    // Spawn event reader
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

    // REPL loop
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
            _ => {
                // Try to parse as JSON
                match serde_json::from_str::<Value>(line) {
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
                }
            }
        }
    }

    // Close WS
    write.send(Message::Close(None)).await?;
    event_handle.abort();
    Ok(())
}

fn expand_home(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path
}


