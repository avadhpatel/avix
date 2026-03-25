# Svc Gap B — Service Process Spawner + `/proc/services/` Status Files

> **Status:** Pending
> **Priority:** Critical
> **Depends on:** Svc gap A (`ServiceUnit` types)
> **Blocks:** Svc gaps C, F, G, H
> **Affects:** `crates/avix-core/src/service/lifecycle.rs`,
>   `crates/avix-core/src/service/process.rs` (new),
>   `crates/avix-core/src/service/status.rs` (new),
>   `crates/avix-core/src/bootstrap/mod.rs` (Phase 4 stub)

---

## Problem

`ServiceManager::spawn_and_get_token` records a service internally but never actually
launches the OS process. The bootstrap `phase3_services()` is `todo!()`. There is no
mechanism to:

- Scan `AVIX_ROOT/services/` for installed `service.unit` files
- Spawn the service binary with the correct environment variables
- Write `/proc/services/<name>/status.yaml` at spawn, register, and stop
- Look up the expected socket path for a service

---

## Scope

Implement OS process spawning driven by `ServiceUnit`, the `/proc/services/<name>/status.yaml`
write path, and Phase 4 bootstrap (scan installed services, spawn each one). No installer.
No restart watchdog (gap H). No tool scanning (gap C).

---

## What Needs to Be Built

### 1. `service/status.rs` — Service status VFS file

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::Pid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Starting,
    Running,
    Degraded,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub version: String,
    pub pid: Pid,
    pub state: ServiceState,
    pub endpoint: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub registered_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub restart_count: u32,
    pub tools: Vec<String>,     // tool names currently registered
}

impl ServiceStatus {
    /// VFS path for this service's status file.
    pub fn vfs_path(name: &str) -> String {
        format!("/proc/services/{name}/status.yaml")
    }
}
```

### 2. `service/process.rs` — `ServiceProcess`

Wraps a spawned `tokio::process::Child` with its associated service metadata.

```rust
use std::path::{Path, PathBuf};
use tokio::process::{Child, Command};
use crate::error::AvixError;
use crate::service::unit::ServiceUnit;
use crate::service::token::ServiceToken;
use crate::types::Pid;

pub struct ServiceProcess {
    pub name: String,
    pub pid: Pid,
    pub child: Child,
    pub socket_path: PathBuf,
}

impl ServiceProcess {
    /// Resolve the IPC socket path for this service.
    pub fn socket_path_for(run_dir: &Path, name: &str, pid: Pid) -> PathBuf {
        #[cfg(unix)]
        { run_dir.join(format!("{name}-{}.sock", pid.as_u32())) }
        #[cfg(windows)]
        { PathBuf::from(format!(r"\\.\pipe\avix-svc-{name}-{}", pid.as_u32())) }
    }

    /// Spawn the service binary described by `unit` with the token env vars injected.
    pub async fn spawn(
        unit: &ServiceUnit,
        token: &ServiceToken,
        kernel_sock: &Path,
        router_sock: &Path,
        run_dir: &Path,
    ) -> Result<Self, AvixError> {
        let pid = token.pid;
        let socket_path = Self::socket_path_for(run_dir, &unit.name, pid);

        let mut cmd = Command::new(&unit.service.binary);
        cmd.envs(build_env(unit, token, kernel_sock, router_sock, &socket_path));
        // Detach from current process group so it isn't killed by Ctrl-C on the CLI.
        #[cfg(unix)]
        cmd.process_group(0);

        let child = cmd.spawn().map_err(|e| AvixError::ConfigParse(
            format!("failed to spawn {}: {e}", unit.service.binary)
        ))?;

        Ok(Self { name: unit.name.clone(), pid, child, socket_path })
    }

    /// True if the child process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

fn build_env(
    unit: &ServiceUnit,
    token: &ServiceToken,
    kernel_sock: &Path,
    router_sock: &Path,
    svc_sock: &Path,
) -> Vec<(String, String)> {
    vec![
        ("AVIX_KERNEL_SOCK".into(), kernel_sock.display().to_string()),
        ("AVIX_ROUTER_SOCK".into(), router_sock.display().to_string()),
        ("AVIX_SVC_SOCK".into(), svc_sock.display().to_string()),
        ("AVIX_SVC_TOKEN".into(), token.token_str.clone()),
    ]
}
```

### 3. `service/lifecycle.rs` — extend `ServiceManager`

Add:

```rust
/// Scan `AVIX_ROOT/services/` for `service.unit` files and return their parsed units.
pub fn discover_installed(root: &Path) -> Result<Vec<ServiceUnit>, AvixError> {
    let services_dir = root.join("services");
    if !services_dir.exists() { return Ok(vec![]); }
    let mut units = Vec::new();
    for entry in std::fs::read_dir(&services_dir)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?
    {
        let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let unit_path = entry.path().join("service.unit");
        if unit_path.exists() {
            units.push(ServiceUnit::load(&unit_path)?);
        }
    }
    Ok(units)
}
```

Also extend `handle_ipc_register` to:
1. Set `record.registered_at` timestamp
2. Write `ServiceStatus` with `state: Running` to the VFS at
   `/proc/services/<name>/status.yaml`

### 4. Bootstrap Phase 4 (`bootstrap/mod.rs`)

Replace the `todo!()` in `phase3_services()`:

```rust
async fn phase3_services(&mut self) -> Result<(), AvixError> {
    let root = &self.root;
    let units = ServiceManager::discover_installed(root)?;
    for unit in units {
        tracing::info!("Starting service: {}", unit.name);
        let token = self.service_manager
            .spawn_and_get_token(ServiceSpawnRequest {
                name: unit.name.clone(),
                binary: unit.service.binary.clone(),
            })
            .await?;
        let proc = ServiceProcess::spawn(
            &unit,
            &token,
            &self.kernel_sock_path,
            &self.router_sock_path,
            &self.run_dir,
        ).await?;
        // Write initial status
        let status = ServiceStatus {
            name: unit.name.clone(),
            version: unit.version.clone(),
            pid: proc.pid,
            state: ServiceState::Starting,
            endpoint: Some(proc.socket_path.display().to_string()),
            started_at: Some(chrono::Utc::now()),
            registered_at: None,
            stopped_at: None,
            restart_count: 0,
            tools: unit.tools.provides.clone(),
        };
        write_status_yaml(&self.vfs, &status).await?;
        self.service_processes.insert(unit.name.clone(), proc);
    }
    Ok(())
}
```

---

## Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ServiceStatus
    #[test]
    fn vfs_path_format() {
        assert_eq!(
            ServiceStatus::vfs_path("github-svc"),
            "/proc/services/github-svc/status.yaml"
        );
    }

    #[test]
    fn service_status_serialises() {
        let status = ServiceStatus {
            name: "test-svc".into(),
            version: "1.0.0".into(),
            pid: Pid::new(42),
            state: ServiceState::Running,
            endpoint: Some("/run/avix/test-svc-42.sock".into()),
            started_at: None,
            registered_at: None,
            stopped_at: None,
            restart_count: 0,
            tools: vec!["test/echo".into()],
        };
        let yaml = serde_yaml::to_string(&status).unwrap();
        assert!(yaml.contains("running"));
        assert!(yaml.contains("test-svc"));
    }

    // ServiceProcess::socket_path_for
    #[test]
    fn socket_path_contains_name_and_pid() {
        let run_dir = std::path::Path::new("/run/avix");
        let path = ServiceProcess::socket_path_for(run_dir, "github-svc", Pid::new(42));
        let s = path.to_string_lossy();
        assert!(s.contains("github-svc"));
        assert!(s.contains("42"));
    }

    // build_env
    #[test]
    fn build_env_contains_all_required_vars() {
        let unit = make_test_unit("echo-svc");
        let token = ServiceToken {
            token_str: "tok-123".into(),
            service_name: "echo-svc".into(),
            pid: Pid::new(5),
        };
        let env = build_env(
            &unit, &token,
            std::path::Path::new("/run/avix/kernel.sock"),
            std::path::Path::new("/run/avix/router.sock"),
            std::path::Path::new("/run/avix/echo-5.sock"),
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AVIX_KERNEL_SOCK"));
        assert!(keys.contains(&"AVIX_ROUTER_SOCK"));
        assert!(keys.contains(&"AVIX_SVC_SOCK"));
        assert!(keys.contains(&"AVIX_SVC_TOKEN"));
        let tok = env.iter().find(|(k, _)| k == "AVIX_SVC_TOKEN").unwrap();
        assert_eq!(tok.1, "tok-123");
    }

    // discover_installed
    #[test]
    fn discover_installed_finds_service_units() {
        let dir = TempDir::new().unwrap();
        let svc_dir = dir.path().join("services").join("my-svc");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(svc_dir.join("service.unit"), r#"
name = "my-svc" version = "1.0.0"
[unit] [service] binary = "/bin/my-svc"
[tools] namespace = "/tools/my/"
"#).unwrap();
        let units = ServiceManager::discover_installed(dir.path()).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].name, "my-svc");
    }

    #[test]
    fn discover_installed_empty_when_no_services_dir() {
        let dir = TempDir::new().unwrap();
        let units = ServiceManager::discover_installed(dir.path()).unwrap();
        assert!(units.is_empty());
    }

    #[test]
    fn discover_installed_skips_dirs_without_unit_file() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("services").join("orphan")).unwrap();
        let units = ServiceManager::discover_installed(dir.path()).unwrap();
        assert!(units.is_empty());
    }
}
```

---

## Dependencies to add

```toml
# avix-core/Cargo.toml
serde_yaml = "0.9"    # if not already present
```

---

## Success Criteria

- [ ] `ServiceStatus::vfs_path` returns correct path pattern
- [ ] `ServiceProcess::socket_path_for` includes name and PID
- [ ] `build_env` injects all four required env vars with correct values
- [ ] `discover_installed` finds `service.unit` files; skips dirs without one
- [ ] `discover_installed` returns empty vec when services dir absent
- [ ] All tests pass; `cargo test --workspace` green
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
