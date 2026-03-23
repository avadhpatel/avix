pub mod phase1;
pub(crate) mod phase2;

use crate::error::AvixError;
use crate::memfs::VfsRouter;
use crate::types::Pid;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BootPhase(pub u8);

#[derive(Debug, Clone)]
pub struct BootLogEntry {
    pub phase: BootPhase,
    pub message: String,
}

pub struct Runtime {
    master_key_set: bool,
    boot_log: Vec<BootLogEntry>,
    service_pids: std::collections::HashMap<String, Pid>,
    vfs: VfsRouter,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("master_key_set", &self.master_key_set)
            .finish()
    }
}

impl Runtime {
    pub async fn bootstrap_with_root(root: &Path) -> Result<Self, AvixError> {
        let mut log = Vec::new();
        let mut service_pids = std::collections::HashMap::new();
        let vfs = VfsRouter::new();

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

        // Phase 2: load master key from env and zero it; mount persistent trees
        let master_key = std::env::var("AVIX_MASTER_KEY")
            .map_err(|_| AvixError::ConfigParse("AVIX_MASTER_KEY env var not set".into()))?;
        // Zero the env var immediately
        std::env::remove_var("AVIX_MASTER_KEY");
        let _key_bytes = master_key.into_bytes(); // held in memory only

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
            master_key_set: true,
            boot_log: log,
            service_pids,
            vfs,
        })
    }

    pub fn vfs(&self) -> &VfsRouter {
        &self.vfs
    }

    pub fn has_master_key(&self) -> bool {
        self.master_key_set
    }

    pub fn boot_log(&self) -> &[BootLogEntry] {
        &self.boot_log
    }

    /// Returns the Pid assigned to a named built-in service, if it was started.
    pub fn service_pid(&self, name: &str) -> Option<Pid> {
        self.service_pids.get(name).copied()
    }
}
