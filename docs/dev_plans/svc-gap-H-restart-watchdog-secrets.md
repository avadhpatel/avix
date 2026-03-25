# Svc Gap H — Restart Watchdog + Service Secrets (`kernel/secret/get`)

> **Status:** Pending
> **Priority:** Medium
> **Depends on:** Svc gaps A (RestartPolicy), B (ServiceProcess)
> **Blocks:** nothing (leaf)
> **Affects:** `crates/avix-core/src/service/watchdog.rs` (new),
>   `crates/avix-core/src/syscall/domain/kernel.rs` (new method),
>   `crates/avix-core/src/secrets/` (extension)

---

## Problem

### Restart watchdog

`RestartPolicy` (on-failure, always, never) is parsed from `service.unit` but never
enforced. If a service crashes, it stays dead. The spec (`service-authoring.md §2`)
defines `restart_delay` too. No process is monitoring service PIDs.

### Service secrets

Services need to call `kernel/secret/get` to retrieve per-user or per-service secrets
(spec §7, §8). There is no `kernel/secret/get` IPC handler. There is also no CLI
`avix secret set --for-service <name>` command.

---

## Scope

Implement a `ServiceWatchdog` background task that monitors service processes and
restarts them according to policy. Implement the `kernel/secret/get` IPC method and
the `avix secret set --for-service <name>` CLI subcommand.

---

## Part 1: Restart Watchdog

### `service/watchdog.rs`

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::error::AvixError;
use crate::service::lifecycle::ServiceManager;
use crate::service::process::ServiceProcess;
use crate::service::unit::{RestartPolicy, ServiceUnit};

pub struct WatchdogEntry {
    pub unit: ServiceUnit,
    pub process: ServiceProcess,
    pub restart_count: u32,
}

pub struct ServiceWatchdog {
    entries: Arc<RwLock<HashMap<String, WatchdogEntry>>>,
    _handle: JoinHandle<()>,
}

impl ServiceWatchdog {
    pub fn start(
        entries: Arc<RwLock<HashMap<String, WatchdogEntry>>>,
        service_manager: Arc<ServiceManager>,
        kernel_sock: std::path::PathBuf,
        router_sock: std::path::PathBuf,
        run_dir: std::path::PathBuf,
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
                ).await;
            }
        });
        Self { entries, _handle: handle }
    }

    pub async fn register(&self, unit: ServiceUnit, process: ServiceProcess) {
        self.entries.write().await.insert(unit.name.clone(), WatchdogEntry {
            unit, process, restart_count: 0,
        });
    }

    pub async fn restart_count(&self, name: &str) -> u32 {
        self.entries.read().await
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
        let (is_running, policy, delay_str) = {
            let mut guard = entries.write().await;
            let Some(entry) = guard.get_mut(&name) else { continue };
            (entry.process.is_running(), entry.unit.service.restart.clone(), entry.unit.service.restart_delay.clone())
        };
        if is_running { continue; }

        let should_restart = match policy {
            RestartPolicy::Always    => true,
            RestartPolicy::OnFailure => true,   // simplified: treat exit as failure
            RestartPolicy::Never     => false,
        };
        if !should_restart { continue; }

        let delay = crate::service::unit::parse_duration(&delay_str)
            .unwrap_or(Duration::from_secs(5));
        warn!("Service {name} died — restarting in {delay:?}");
        tokio::time::sleep(delay).await;

        let (unit, token) = {
            let guard = entries.read().await;
            let entry = guard.get(&name).unwrap();
            (entry.unit.clone(), manager.respawn_token(&entry.unit.name).await.unwrap())
        };
        match ServiceProcess::spawn(&unit, &token, kernel_sock, router_sock, run_dir).await {
            Ok(new_proc) => {
                info!("Service {name} restarted — new pid={}", new_proc.pid.as_u32());
                let mut guard = entries.write().await;
                if let Some(entry) = guard.get_mut(&name) {
                    entry.process = new_proc;
                    entry.restart_count += 1;
                }
            }
            Err(e) => warn!("Failed to restart {name}: {e}"),
        }
    }
}
```

Add `ServiceManager::respawn_token` — re-issues a new `ServiceToken` for the same
service name with a fresh PID:

```rust
pub async fn respawn_token(&self, name: &str) -> Result<ServiceToken, AvixError> {
    self.spawn_and_get_token(ServiceSpawnRequest {
        name: name.to_string(),
        binary: String::new(),   // binary comes from unit, not manager
        caller_scoped: false,
        max_concurrent: 20,
    }).await
}
```

---

## Part 2: `kernel/secret/get` IPC Method

### `secrets/` extension

The existing secrets module encrypts/decrypts blobs using `AVIX_MASTER_KEY`. Extend it
with a `SecretStore::get_for_service` method:

```rust
// secrets/mod.rs or secrets/store.rs
impl SecretStore {
    /// Retrieve and decrypt a secret for `owner` (e.g. "user:alice", "service:github-svc").
    /// Returns the plaintext value — never writes it anywhere.
    pub fn get(&self, owner: &str, name: &str) -> Result<String, AvixError> { ... }

    /// Store an encrypted secret for `owner`.
    pub fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), AvixError> { ... }
}
```

Secrets for services are stored at `AVIX_ROOT/secrets/<owner-type>/<owner-name>/<name>.enc`.

### `kernel/secret/get` syscall

```rust
// syscall/domain/kernel.rs
pub async fn handle_secret_get(
    ctx: &SyscallContext,
    body: serde_json::Value,
    secret_store: &SecretStore,
) -> SyscallResult {
    // Validate that the service token has access to secrets for this owner.
    // For now: a service may read its own secrets ("service:<name>") unconditionally,
    // and per-user secrets ("user:<name>") only if it has the caller_scoped grant.
    let owner = body["owner"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing `owner`".into()))?;
    let name = body["name"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing `name`".into()))?;

    // Permission check
    if owner.starts_with("service:") {
        let svc_name = owner.trim_start_matches("service:");
        if svc_name != ctx.token.service_name_hint.as_deref().unwrap_or("") {
            return Err(SyscallError::Eperm(ctx.caller_pid, "kernel/secret/get".into()));
        }
    }

    let value = secret_store.get(owner, name)
        .map_err(|e| SyscallError::Enoent(e.to_string()))?;

    Ok(serde_json::json!({ "value": value }))
}
```

### CLI: `avix secret set <name> <value> --for-service <name>`

Add to `avix-cli`:

```
avix secret set <secret-name> <value> --for-service <service-name>
avix secret set <secret-name> <value> --for-user <username>
avix secret list [--for-service <name>] [--for-user <name>]
avix secret delete <secret-name> --for-service <name>
```

These write directly to `AVIX_ROOT/secrets/` using `SecretStore::set` (no ATP needed —
they are admin operations that require filesystem access to the root).

---

## Tests

### Watchdog tests

```rust
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
            RestartPolicy::Always    => true,
            RestartPolicy::OnFailure => true,
            RestartPolicy::Never     => false,
        };
        assert!(!should);
    }

    #[test]
    fn always_policy_restarts() {
        let should = match RestartPolicy::Always {
            RestartPolicy::Always    => true,
            RestartPolicy::OnFailure => true,
            RestartPolicy::Never     => false,
        };
        assert!(should);
    }
}
```

### Secret store tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn set_and_get_service_secret_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = SecretStore::new(dir.path(), b"test-master-key-32-bytes-padded!!");
        store.set("service:github-svc", "app-key", "ghp_test123").unwrap();
        let value = store.get("service:github-svc", "app-key").unwrap();
        assert_eq!(value, "ghp_test123");
    }

    #[test]
    fn get_nonexistent_secret_errors() {
        let dir = TempDir::new().unwrap();
        let store = SecretStore::new(dir.path(), b"test-master-key-32-bytes-padded!!");
        assert!(store.get("service:x", "missing").is_err());
    }

    #[test]
    fn secrets_are_not_stored_in_plaintext() {
        let dir = TempDir::new().unwrap();
        let store = SecretStore::new(dir.path(), b"test-master-key-32-bytes-padded!!");
        store.set("service:svc", "key", "supersecret").unwrap();
        // The stored file must NOT contain the plaintext value
        let content = std::fs::read_to_string(
            dir.path().join("service").join("svc").join("key.enc")
        ).unwrap();
        assert!(!content.contains("supersecret"));
    }
}
```

### CLI secret tests

```rust
#[test]
fn secret_set_for_service_parses() {
    let cli = Cli::try_parse_from([
        "avix", "secret", "set", "github-app-key", "ghp_abc",
        "--for-service", "github-svc",
    ]).unwrap();
    // verify subcommand fields
}
```

---

## Success Criteria

- [ ] `ServiceWatchdog` monitors processes every 5 s; restarts on exit with `on-failure` or `always` policy
- [ ] `never` policy: process is not restarted
- [ ] `restart_count` increments on each restart
- [ ] `SecretStore::set` stores encrypted (not plaintext) values on disk
- [ ] `SecretStore::get` decrypts and returns plaintext only in memory
- [ ] `kernel/secret/get` IPC method enforces service-name match for service-owned secrets
- [ ] `avix secret set --for-service <name>` CLI command parses and writes secret
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
