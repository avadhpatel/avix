use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::sync::Arc;

use avix_core::bootstrap::Runtime;
use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
use avix_core::cli::config_reload::{run_config_reload, ReloadParams};
use avix_core::cli::resolve::{run_resolve, ResolveParams};
use avix_core::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
use avix_core::executor::spawn::SpawnParams;
use avix_core::llm_client::LlmClient;
use avix_core::llm_svc::autoagents_client::AutoAgentsChatClient;
// TODO: in daemon mode use IpcLlmClient to call a running llm.svc
use avix_core::types::token::CapabilityToken;
use avix_core::types::Pid;
#[allow(unused_imports)]
use avix_core::IpcLlmClient;

#[derive(Parser)]
#[command(name = "avix", about = "Avix agent OS", version)]
struct Cli {
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
        /// LLM provider to use
        #[arg(long, default_value = "anthropic")]
        provider: LlmProviderArg,
        /// Model name (uses provider default if omitted)
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
}

#[derive(Clone, ValueEnum)]
enum LlmProviderArg {
    Anthropic,
    Openai,
    Ollama,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Cmd::Config {
            sub: ConfigCmd::Init { root, user, role },
        } => {
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
                "  AVIX_MASTER_KEY=<32-char-key> ANTHROPIC_API_KEY=<key> \\\n  avix run --root {} --goal \"say hello world\"",
                root.display()
            );
        }

        Cmd::Config {
            sub: ConfigCmd::Reload { check, root },
        } => {
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
            provider,
            model,
        } => {
            let root = expand_home(root);

            // Build the LLM client via AutoAgents — type-erased as Box<dyn LlmClient>
            let llm_client: Box<dyn LlmClient> = build_llm_client(provider, model)?;

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
                selected_model: "claude-sonnet-4".into(),
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
    }

    Ok(())
}

fn build_llm_client(provider: LlmProviderArg, model: Option<String>) -> Result<Box<dyn LlmClient>> {
    match provider {
        LlmProviderArg::Anthropic => {
            use autoagents::llm::backends::anthropic::Anthropic;
            use autoagents::llm::builder::LLMBuilder;

            let api_key =
                std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")?;
            let m = model.unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
            let p = LLMBuilder::<Anthropic>::new()
                .api_key(api_key)
                .model(m)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        LlmProviderArg::Openai => {
            use autoagents::llm::backends::openai::OpenAI;
            use autoagents::llm::builder::LLMBuilder;

            let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY not set")?;
            let m = model.unwrap_or_else(|| "gpt-4.1-nano".to_string());
            let p = LLMBuilder::<OpenAI>::new()
                .api_key(api_key)
                .model(m)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
        LlmProviderArg::Ollama => {
            use autoagents::llm::backends::ollama::Ollama;
            use autoagents::llm::builder::LLMBuilder;

            let m = model.unwrap_or_else(|| "llama3.2".to_string());
            let p = LLMBuilder::<Ollama>::new()
                .model(m)
                .max_tokens(4096)
                .build()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Box::new(AutoAgentsChatClient::new(p)))
        }
    }
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
