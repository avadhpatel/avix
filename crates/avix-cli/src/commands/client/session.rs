use std::io::Write as _;

use anyhow::Result;
use clap::Subcommand;

use avix_client_core::atp::types::Cmd as AtpCmd_;

use crate::util::{connect_config, emit};

#[derive(Subcommand)]
pub enum SessionCmd {
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
    /// Delete a session record
    Delete {
        /// Session ID
        session_id: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

pub async fn run(sub: SessionCmd, json: bool) -> Result<()> {
    match sub {
        SessionCmd::List { username, status } => {
            let dispatcher = connect_config(None, None).await?;
            let username = username.as_deref().unwrap_or("");
            let reply = dispatcher
                .call(&AtpCmd_::new(
                    "proc",
                    "session-list",
                    &dispatcher.token,
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
                json,
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
                    &dispatcher.token,
                    serde_json::json!({ "id": session_id }),
                ))
                .await?;
            if !reply.ok {
                anyhow::bail!(reply.message.unwrap_or_else(|| "get session failed".into()));
            }
            let body = reply.body.unwrap_or(serde_json::json!({}));
            emit(
                json,
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
                    &dispatcher.token,
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
                json,
                |b: &&serde_json::Value| {
                    format!("Resumed session, PID: {}", b["pid"].as_u64().unwrap_or(0))
                },
                &body,
            );
        }

        SessionCmd::Delete { session_id, force } => {
            if !force {
                print!("Delete session {}? [y/N] ", session_id);
                std::io::stdout().flush().ok();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).ok();
                if !input.trim().eq_ignore_ascii_case("y") {
                    emit(json, |_: &()| "Aborted".to_string(), ());
                    return Ok(());
                }
            }
            let dispatcher = connect_config(None, None).await?;
            let reply = dispatcher
                .call(&AtpCmd_::new(
                    "proc",
                    "session-delete",
                    &dispatcher.token,
                    serde_json::json!({ "session_id": session_id }),
                ))
                .await?;
            if !reply.ok {
                anyhow::bail!(reply
                    .message
                    .unwrap_or_else(|| "delete session failed".into()));
            }
            emit(
                json,
                |_: &()| format!("Deleted session {}", session_id),
                (),
            );
        }
    }

    Ok(())
}
