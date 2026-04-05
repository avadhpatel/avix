use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;
use std::sync::Arc;

use avix_core::bootstrap::Runtime;
use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
use avix_core::cli::config_reload::{run_config_reload, ReloadParams};
use avix_core::cli::resolve::{run_resolve, ResolveParams};
use avix_core::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
use avix_core::executor::spawn::SpawnParams;
use avix_core::llm_client::LlmClient;
use avix_core::types::token::CapabilityToken;
use avix_core::types::Pid;
// TODO: in daemon mode use IpcLlmClient to call a running llm.svc
#[allow(unused_imports)]
use avix_core::IpcLlmClient;

use crate::util::{build_llm_client, default_text_model, expand_home, load_llm_config};

#[derive(Subcommand)]
pub enum ServerCmd {
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
pub enum ServerConfigCmd {
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

pub async fn run(sub: ServerCmd) -> Result<()> {
    match sub {
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
            use avix_core::types::Modality;

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
    }

    Ok(())
}
