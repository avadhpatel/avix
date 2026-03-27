pub mod phase1;
pub(crate) mod phase2;

use crate::auth::atp_token::ATPTokenStore;
use crate::auth::service::AuthService;
use crate::error::AvixError;
use crate::gateway::config::GatewayConfig;
use crate::gateway::event_bus::AtpEventBus;
use crate::gateway::server::GatewayServer;
use crate::kernel::phase3_re_adopt;
use crate::memfs::{VfsPath, VfsRouter};
use crate::process::table::ProcessTable;
use crate::types::Pid;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BootPhase(pub u8);

#[derive(Debug, Clone)]
pub struct BootLogEntry {
    pub phase: BootPhase,
    pub message: String,
}

pub struct Runtime {
    master_key: Arc<String>,
    boot_log: Vec<BootLogEntry>,
    service_pids: std::collections::HashMap<String, Pid>,
    vfs: VfsRouter,
    process_table: Arc<ProcessTable>,
    root: PathBuf,
    runtime_dir: PathBuf,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("master_key_set", &true)
            .finish()
    }
}

impl Runtime {
    pub async fn bootstrap_with_root(root: &Path) -> Result<Self, AvixError> {
        let mut log = Vec::new();
        let mut service_pids = std::collections::HashMap::new();
        let vfs = VfsRouter::new();
        let process_table = Arc::new(ProcessTable::new());
        let runtime_dir = std::env::var("AVIX_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| root.join("run/avix"));

        // Phase 0: init
        log.push(BootLogEntry {
            phase: BootPhase(0),
            message: "phase 0: init".into(),
        });

        // Phase 1: check auth.conf + VFS skeleton (ephemeral /kernel/ and /proc/)
        let auth_conf = root.join("etc/auth.conf");
        if !auth_conf.exists() {
            return Err(AvixError::ConfigParse(
                "auth.conf not found — run `avix config init` first".into(),
            ));
        }
        phase1::run(&vfs).await;
        log.push(BootLogEntry {
            phase: BootPhase(1),
            message: "phase 1: VFS mount".into(),
        });

        // Phase 2: load signing key from etc/signing.key (written by `avix server config init`)
        let signing_key_path = root.join("etc/signing.key");
        let master_key = std::fs::read_to_string(&signing_key_path)
            .map_err(|_| AvixError::ConfigParse(
                "etc/signing.key not found — run `avix server config init` first".into(),
            ))?
            .trim()
            .to_string();

        phase2::mount_persistent_trees(&vfs, root).await?;
        log.push(BootLogEntry {
            phase: BootPhase(2),
            message: "phase 2: config + master key + persistent mounts".into(),
        });

        // Phase 3: start built-in services
        let builtins = [
            "logger",
            "memfs",
            "auth",
            "router",
            "tool-registry",
            "llm",
            "exec",
            "mcp-bridge",
            "gateway",
        ];
        for (i, svc) in builtins.iter().enumerate() {
            service_pids.insert(svc.to_string(), Pid::new((i + 1) as u32));
        }
        log.push(BootLogEntry {
            phase: BootPhase(3),
            message: "phase 3: services started".into(),
        });

        Ok(Runtime {
            master_key: Arc::new(master_key),
            boot_log: log,
            service_pids,
            vfs,
            process_table,
            root: root.to_path_buf(),
            runtime_dir,
        })
    }

    pub fn vfs(&self) -> &VfsRouter {
        &self.vfs
    }

    pub fn has_master_key(&self) -> bool {
        true
    }

    pub fn boot_log(&self) -> &[BootLogEntry] {
        &self.boot_log
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Returns the Pid assigned to a named built-in service, if it was started.
    pub fn service_pid(&self, name: &str) -> Option<Pid> {
        self.service_pids.get(name).copied()
    }

    /// Starts the daemon: spawns kernel.agent, services, ATP gateway, and polls for hot reload.
    pub async fn start_daemon(mut self, port: u16, test_mode: bool) -> Result<(), AvixError> {
        // Phase 2: spawn kernel.agent PID1
        self.phase2_kernel().await?;
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(2),
            message: "phase 2: kernel.agent spawned".into(),
        });

        // Phase 3: spawn services
        self.phase3_services().await?;
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(3),
            message: "phase 3: services spawned".into(),
        });

        // Phase 3.5: re-adopt orphaned agents
        let agents_yaml_path = self.root.join("etc/avix/agents.yaml");
        let master_key_bytes = hex::decode(&*self.master_key).map_err(|e| AvixError::ConfigParse(format!("invalid master key: {}", e)))?;
        phase3_re_adopt(self.process_table.clone(), agents_yaml_path, master_key_bytes).await?;
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(3),
            message: "phase 3.5: re-adopted agents".into(),
        });

        // Phase 4: start ATP gateway
        self.phase4_atp_gateway(port, test_mode).await?;
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(4),
            message: "phase 4: ATP gateway started".into(),
        });

        // Poll reload-pending every 5s
        loop {
            time::sleep(Duration::from_secs(5)).await;
            if fs::metadata("/run/avix/reload-pending").is_ok() {
                self.hot_reload().await?;
            }
        }
    }

    async fn phase2_kernel(&mut self) -> Result<(), AvixError> {
        tracing::info!("kernel mock PID1");
        Ok(())
    }

    async fn phase3_services(&mut self) -> Result<(), AvixError> {
        tracing::info!("services mock");
        Ok(())
    }

    async fn phase4_atp_gateway(&mut self, port: u16, test_mode: bool) -> Result<(), AvixError> {
        let auth_yaml = self
            .vfs
            .read(&VfsPath::parse("/etc/avix/auth.conf")?)
            .await?;
        let auth_config: crate::config::auth::AuthConfig = serde_yaml::from_slice(&auth_yaml)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let auth_svc = Arc::new(AuthService::new(auth_config));
        let token_store = Arc::new(ATPTokenStore::new(self.master_key.as_ref().clone()));
        let event_bus = Arc::new(AtpEventBus::default());
        let config = GatewayConfig {
            user_addr: format!("0.0.0.0:{}", port)
                .parse()
                .map_err(|_| AvixError::ConfigParse("invalid port".into()))?,
            admin_addr: "127.0.0.1:7701".parse().unwrap(),
            tls_enabled: false,
            hil_timeout_secs: 600,
            kernel_sock: std::env::var("AVIX_KERNEL_SOCK").ok().map(PathBuf::from),
        };
        let _user_addr = config.user_addr;
        let server = GatewayServer::new(config, auth_svc, token_store, event_bus);
        tokio::spawn(async move {
            let _ = server.run(test_mode).await;
        });
        Ok(())
    }

    async fn hot_reload(&mut self) -> Result<(), AvixError> {
        tracing::info!("reload stub");
        Ok(())
    }
}
