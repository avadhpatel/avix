use anyhow::Result;
use clap::Subcommand;

use avix_client_core::commands::resolve_hil;

use crate::util::{connect_config, emit};

#[derive(Subcommand)]
pub enum HilCmd {
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

pub async fn run(sub: HilCmd, json: bool) -> Result<()> {
    match sub {
        HilCmd::Approve {
            pid,
            hil_id,
            token,
            note,
        } => {
            let dispatcher = connect_config(None, None).await?;
            resolve_hil(&dispatcher, pid, &hil_id, &token, true, note.as_deref()).await?;
            emit(
                json,
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
                json,
                |_: &()| format!("Denied HIL {} for PID {}", hil_id, pid),
                (),
            );
        }
    }

    Ok(())
}
