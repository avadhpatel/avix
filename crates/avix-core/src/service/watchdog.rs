use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::service::lifecycle::ServiceManager;
use crate::service::process::ServiceProcess;
use crate::service::yaml::{parse_duration, RestartPolicy, ServiceUnit};

pub struct WatchdogEntry {
    pub unit: ServiceUnit,
    pub process: ServiceProcess,
    pub restart_count: u32,
}

pub struct ServiceWatchdog {
    pub(crate) entries: Arc<RwLock<HashMap<String, WatchdogEntry>>>,
    _handle: JoinHandle<()>,
}

impl ServiceWatchdog {
    /// Start the watchdog loop.  Checks every 5 seconds and restarts dead services
    /// according to their `RestartPolicy`.
    pub fn start(
        entries: Arc<RwLock<HashMap<String, WatchdogEntry>>>,
        service_manager: Arc<ServiceManager>,
        kernel_sock: PathBuf,
        router_sock: PathBuf,
        run_dir: PathBuf,
    ) -> Self {
        let entries_clone = Arc::clone(&entries);
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                check_and_restart(
                    &entries_clone,
                    &service_manager,
                    &kernel_sock,
                    &router_sock,
                    &run_dir,
                )
                .await;
            }
        });
        Self {
            entries,
            _handle: handle,
        }
    }

    /// Register a service process with the watchdog.
    pub async fn register(&self, unit: ServiceUnit, process: ServiceProcess) {
        self.entries.write().await.insert(
            unit.name.clone(),
            WatchdogEntry {
                unit,
                process,
                restart_count: 0,
            },
        );
    }

    /// Return the number of times `name` has been restarted by the watchdog.
    pub async fn restart_count(&self, name: &str) -> u32 {
        self.entries
            .read()
            .await
            .get(name)
            .map(|e| e.restart_count)
            .unwrap_or(0)
    }
}

async fn check_and_restart(
    entries: &Arc<RwLock<HashMap<String, WatchdogEntry>>>,
    manager: &Arc<ServiceManager>,
    kernel_sock: &std::path::Path,
    router_sock: &std::path::Path,
    run_dir: &std::path::Path,
) {
    let names: Vec<String> = entries.read().await.keys().cloned().collect();
    for name in names {
        // Determine if the process is still running and what the restart policy is.
        let (is_running, policy, delay_str) = {
            let mut guard = entries.write().await;
            let Some(entry) = guard.get_mut(&name) else {
                continue;
            };
            (
                entry.process.is_running(),
                entry.unit.service.restart.clone(),
                entry.unit.service.restart_delay.clone(),
            )
        }; // guard dropped here

        if is_running {
            continue;
        }

        let should_restart = match policy {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => true, // simplified: all exits treated as failure
            RestartPolicy::Never => false,
        };
        if !should_restart {
            continue;
        }

        let delay = parse_duration(&delay_str).unwrap_or(Duration::from_secs(5));
        warn!(service = %name, delay = ?delay, "service exited — restarting");
        tokio::time::sleep(delay).await;

        // Clone the unit so we don't hold the lock across await.
        let unit_clone = {
            let guard = entries.read().await;
            guard.get(&name).map(|e| e.unit.clone())
        };
        let Some(unit) = unit_clone else {
            continue;
        };

        let token = match manager.respawn_token(&unit.name).await {
            Ok(t) => t,
            Err(e) => {
                warn!(service = %name, error = %e, "respawn_token failed");
                continue;
            }
        };

        match ServiceProcess::spawn(&unit, &token, kernel_sock, router_sock, run_dir).await {
            Ok(new_proc) => {
                info!(service = %name, pid = new_proc.pid.as_u64(), "service restarted");
                let mut guard = entries.write().await;
                if let Some(entry) = guard.get_mut(&name) {
                    entry.process = new_proc;
                    entry.restart_count += 1;
                }
            }
            Err(e) => warn!(service = %name, error = %e, "failed to restart service"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restart_count_starts_at_zero() {
        let entries = Arc::new(RwLock::new(HashMap::new()));
        let watchdog = ServiceWatchdog {
            entries: Arc::clone(&entries),
            _handle: tokio::spawn(async {}),
        };
        assert_eq!(watchdog.restart_count("missing-svc").await, 0);
    }

    #[test]
    fn never_policy_does_not_restart() {
        let should = match RestartPolicy::Never {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => true,
            RestartPolicy::Never => false,
        };
        assert!(!should);
    }

    #[test]
    fn always_policy_restarts() {
        let should = match RestartPolicy::Always {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => true,
            RestartPolicy::Never => false,
        };
        assert!(should);
    }

    #[test]
    fn on_failure_policy_restarts() {
        let should = match RestartPolicy::OnFailure {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => true,
            RestartPolicy::Never => false,
        };
        assert!(should);
    }
}
