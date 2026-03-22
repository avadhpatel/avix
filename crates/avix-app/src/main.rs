mod atp_client;

use atp_client::{AgentCommand, AgentCommandError, AtpEvent};

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("avix-app starting");
    // Demonstrate the core ATP types compile and are usable
    let cmd = AgentCommand::Spawn {
        name: "kernel.agent".into(),
        goal: "boot".into(),
    };
    let _json = cmd.to_json();
    let _err: Option<AgentCommandError> = None;
    let _event = AtpEvent::from_json(&serde_json::json!({}));
}
