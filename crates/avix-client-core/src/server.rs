use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{info, warn};

use crate::config::ClientConfig;
use crate::error::ClientError;

/// Manages an optionally-owned `avix start` child process.
///
/// If the server is already running when `ensure_running` is called, no child
/// is spawned and `is_alive` always returns `true` (we didn't start it).
pub struct ServerHandle {
    /// `Some` only when *we* spawned the server; `None` when it was pre-running.
    child: Arc<Mutex<Option<Child>>>,
    /// Whether we own the process (false → server was already up).
    owned: bool,
}

impl std::fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerHandle")
            .field("owned", &self.owned)
            .finish_non_exhaustive()
    }
}

impl ServerHandle {
    /// Probe the server URL. If reachable, return immediately without spawning.
    /// If not reachable and `config.auto_start_server` is true, spawn `avix start`
    /// and wait for the server to come up (5 probes × 500 ms).
    pub async fn ensure_running(config: &ClientConfig) -> Result<Self, ClientError> {
        if probe_reachable(&config.server_url).await {
            info!("Server already reachable at {}", config.server_url);
            return Ok(Self {
                child: Arc::new(Mutex::new(None)),
                owned: false,
            });
        }

        if !config.auto_start_server {
            return Err(ClientError::Other(anyhow::anyhow!(
                "Server not reachable at {} and auto_start_server is disabled",
                config.server_url
            )));
        }

        let avix_bin = std::env::current_exe()
            .map_err(|e| ClientError::Other(anyhow::anyhow!("Cannot find avix binary: {e}")))?;

        info!(
            "Spawning server: {} start --root {:?}",
            avix_bin.display(),
            config.runtime_root
        );

        let mut cmd = Command::new(&avix_bin);
        cmd.arg("start").arg("--root").arg(&config.runtime_root);

        // Forward AVIX_MASTER_KEY if present in the environment.
        if let Ok(key) = std::env::var("AVIX_MASTER_KEY") {
            cmd.env("AVIX_MASTER_KEY", key);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ClientError::Other(anyhow::anyhow!("Failed to spawn avix start: {e}")))?;

        // Wait for the server to become reachable (up to 5 × 500 ms = 2.5 s).
        let mut reachable = false;
        for attempt in 1..=5 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if probe_reachable(&config.server_url).await {
                info!("Server reachable after {} probe(s)", attempt);
                reachable = true;
                break;
            }
            warn!(
                "Probe {attempt}/5: server not yet reachable at {}",
                config.server_url
            );
        }

        if !reachable {
            return Err(ClientError::Other(anyhow::anyhow!(
                "Server did not become reachable after 5 probes"
            )));
        }

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            owned: true,
        })
    }

    /// True if the process we spawned is still running.
    /// Always returns `true` when we did not spawn the server ourselves.
    pub fn is_alive(&self) -> bool {
        if !self.owned {
            return true;
        }
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            // `try_wait` returns `Ok(None)` if still running, `Ok(Some(_))` if exited.
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    /// Kill the child process if we own it. No-op if we did not spawn the server.
    pub fn stop(&self) -> Result<(), ClientError> {
        if !self.owned {
            return Ok(());
        }
        let mut guard = self.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            child
                .kill()
                .map_err(|e| ClientError::Other(anyhow::anyhow!("Failed to kill server: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Intentionally do NOT kill the server on drop — it should outlive the client.
    }
}

/// Returns true if the server URL responds to an HTTP request (any status, including 401).
/// Returns false only on connection-refused / network errors.
async fn probe_reachable(server_url: &str) -> bool {
    let url = format!("{server_url}/atp/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(400))
        .build()
        .unwrap_or_default();
    match client.get(&url).send().await {
        Ok(_) => true,                     // any HTTP response → server is up
        Err(e) if e.is_connect() => false, // connection refused
        Err(_) => false,                   // other errors (timeout, DNS) → treat as down
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server handle that does not own a process is always considered alive.
    #[test]
    fn non_owned_handle_is_alive() {
        let handle = ServerHandle {
            child: Arc::new(Mutex::new(None)),
            owned: false,
        };
        assert!(handle.is_alive());
    }

    /// A handle owning a process that has already exited reports not alive.
    #[test]
    fn owned_handle_exited_process_not_alive() {
        // Spawn a process that exits immediately.
        let child = Command::new("true").spawn().unwrap();
        let handle = ServerHandle {
            child: Arc::new(Mutex::new(Some(child))),
            owned: true,
        };
        // Give the process a moment to exit.
        std::thread::sleep(Duration::from_millis(50));
        assert!(!handle.is_alive());
    }

    /// Stopping a non-owned handle is a no-op and does not error.
    #[test]
    fn stop_non_owned_is_noop() {
        let handle = ServerHandle {
            child: Arc::new(Mutex::new(None)),
            owned: false,
        };
        assert!(handle.stop().is_ok());
    }

    /// Stopping an owned running process succeeds.
    #[test]
    fn stop_owned_running_process() {
        // `sleep 60` gives us a long-lived target to kill.
        let child = Command::new("sleep").arg("60").spawn().unwrap();
        let handle = ServerHandle {
            child: Arc::new(Mutex::new(Some(child))),
            owned: true,
        };
        assert!(handle.stop().is_ok());
    }
}
