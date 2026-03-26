//! avix-re: RuntimeExecutor binary for Avix agents.
//!
//! This binary runs an LLM agent process. It parses environment variables,
//! registers Category 2 tools via IPC, and executes the agent's goal using the LLM.
//!
//! Environment variables:
//! - AVIX_PID: Process ID (u32)
//! - AVIX_GOAL: Agent goal (string)
//! - AVIX_TOKEN: Serialized CapabilityToken (JSON)
//! - AVIX_SESSION_ID: Session ID (string)
//! - AVIX_AGENT_NAME: Agent name (string)
//! - AVIX_SPAWNED_BY: Username who spawned (string)
//! - AVIX_MASTER_KEY: Master key for decryption (optional)
//! - AVIX_KERNEL_SOCK: IPC socket path (optional, defaults to env)
//!
//! Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance

use anyhow::{Context, Result};
use avix_core::executor::runtime_executor::RuntimeExecutor;
use avix_core::executor::spawn::SpawnParams;
use avix_core::llm_client::LlmClient;
use avix_core::types::token::CapabilityToken;
use avix_core::types::Pid;
use std::env;
use tracing::{info, error};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (simple for now)
    tracing_subscriber::fmt::init();

    info!("avix-re starting");

    // Parse environment variables
    let pid: u32 = env::var("AVIX_PID")
        .context("AVIX_PID not set")?
        .parse()
        .context("Invalid AVIX_PID")?;
    let goal = env::var("AVIX_GOAL").context("AVIX_GOAL not set")?;
    let token_json = env::var("AVIX_TOKEN").context("AVIX_TOKEN not set")?;
    let session_id = env::var("AVIX_SESSION_ID").context("AVIX_SESSION_ID not set")?;
    let agent_name = env::var("AVIX_AGENT_NAME").context("AVIX_AGENT_NAME not set")?;
    let spawned_by = env::var("AVIX_SPAWNED_BY").context("AVIX_SPAWNED_BY not set")?;

    let token: CapabilityToken = serde_json::from_str(&token_json)
        .context("Invalid AVIX_TOKEN JSON")?;

    info!(pid, agent_name, goal, "parsed environment");

    // TODO: Create IPC client and register Category 2 tools via ipc.tool-add
    // For now, stub: assume tools are registered

    // Create SpawnParams
    let params = SpawnParams {
        pid: Pid::new(pid),
        agent_name: agent_name.clone(),
        goal: goal.clone(),
        spawned_by: spawned_by.clone(),
        session_id: session_id.clone(),
        token: token.clone(),
        system_prompt: None,
        selected_model: "claude-sonnet-4".to_string(), // TODO: from config
        denied_tools: vec![],
        context_limit: 0,
    };

    // TODO: Create LLM client (anthropic or openai)
    // For now, stub: use a mock or panic
    let llm_client: Box<dyn LlmClient> = todo!("Implement LLM client instantiation");

    // Create RuntimeExecutor
    // Note: In production, this would be done with registry and kernel handles,
    // but for avix-re, we need to set up IPC-based registry.
    // For now, stub
    let executor: RuntimeExecutor = todo!("Create RuntimeExecutor with IPC registry");

    // Run the agent loop
    let result = executor.run_with_client(&goal, llm_client.as_ref()).await;

    match result {
        Ok(turn_result) => {
            info!(pid, "agent completed: {}", turn_result.text);
            println!("{}", turn_result.text); // Output to stdout for capture
        }
        Err(e) => {
            error!(pid, error = ?e, "agent failed");
            eprintln!("Error: {}", e); // Error to stderr
            std::process::exit(1);
        }
    }

    Ok(())
}