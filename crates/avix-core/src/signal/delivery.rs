/// Signal delivery over IPC.
///
/// Signals are delivered as JSON-RPC notifications (no `id` field) to the
/// per-agent socket at `/run/avix/agents/<pid>.sock` (spec §7).
use crate::error::AvixError;
use crate::ipc::{message::JsonRpcNotification, platform, IpcClient};
use crate::signal::kind::{Signal, SignalKind};
use crate::types::Pid;
use serde_json::json;
use std::path::PathBuf;

pub struct SignalDelivery {
    run_dir: PathBuf,
}

impl SignalDelivery {
    pub fn new(run_dir: PathBuf) -> Self {
        Self { run_dir }
    }

    /// Deliver a signal to a specific agent's IPC socket.
    ///
    /// Sends a JSON-RPC notification (no `id`) to `/run/avix/agents/<pid>.sock`.
    /// Returns `Err(AvixError::NotFound)` if the socket does not exist.
    pub async fn deliver(&self, signal: Signal) -> Result<(), AvixError> {
        let sock = platform::agent_sock_path(&self.run_dir, signal.target);
        if !sock.exists() {
            return Err(AvixError::NotFound(format!(
                "agent socket for pid {} not found at {}",
                signal.target,
                sock.display()
            )));
        }
        let client = IpcClient::new(sock);
        let notif = JsonRpcNotification::new(
            "signal",
            json!({
                "signal": signal.kind.as_str(),
                "payload": signal.payload,
            }),
        );
        client.notify(notif).await.map_err(|e| {
            AvixError::NotFound(format!(
                "failed to deliver signal to pid {}: {e}",
                signal.target
            ))
        })
    }

    /// Broadcast a signal to all listed PIDs concurrently.
    ///
    /// Returns one result per PID. Missing sockets produce `Err(NotFound)` but
    /// do not stop delivery to remaining PIDs.
    pub async fn broadcast(
        &self,
        pids: &[Pid],
        kind: SignalKind,
        payload: serde_json::Value,
    ) -> Vec<(Pid, Result<(), AvixError>)> {
        let mut join_set = tokio::task::JoinSet::new();
        for &pid in pids {
            let signal = Signal {
                target: pid,
                kind: kind.clone(),
                payload: payload.clone(),
            };
            let run_dir = self.run_dir.clone();
            join_set.spawn(async move {
                let delivery = SignalDelivery { run_dir };
                (pid, delivery.deliver(signal).await)
            });
        }
        let mut results = Vec::new();
        while let Some(res) = join_set.join_next().await {
            results.push(res.expect("signal delivery task panicked"));
        }
        results
    }
}
