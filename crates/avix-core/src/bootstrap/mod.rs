pub mod executor_factory;
pub mod phase1;
pub(crate) mod phase2;

use crate::agent_manifest::scanner::ManifestScanner;
use crate::auth::atp_token::ATPTokenStore;
use crate::auth::service::AuthService;
use crate::config::LlmConfig;
use crate::error::AvixError;
use crate::invocation::InvocationStore;
use crate::session::PersistentSessionStore;
use crate::exec_svc::ExecIpcServer;
use crate::gateway::config::GatewayConfig;
use crate::gateway::event_bus::AtpEventBus;
use crate::gateway::server::GatewayServer;
use crate::kernel::{phase3_crash_recovery, KernelIpcServer, ProcHandler};
use crate::llm_svc::routing::RoutingEngine;
use crate::llm_svc::LlmIpcServer;
use crate::mcp_bridge::{McpBridgeRunner, McpConfig};
use crate::memfs::{VfsPath, VfsRouter};
use crate::process::table::ProcessTable;
use crate::router::{RouterDispatcher, RouterIpcServer, ServiceRegistry};
use crate::service::lifecycle::{ServiceManager, ServiceSpawnRequest};
use crate::service::process::ServiceProcess;
use crate::service::watchdog::{ServiceWatchdog, WatchdogEntry};
use crate::signal::SignalChannelRegistry;
use crate::trace::{TraceFlags, Tracer};
use crate::types::Pid;
use std::collections::HashMap;
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
    vfs: Arc<VfsRouter>,
    process_table: Arc<ProcessTable>,
    root: PathBuf,
    runtime_dir: PathBuf,
    kernel_sock: PathBuf,
    /// Shared event bus — created at bootstrap so phase2_kernel and phase4_atp_gateway
    /// both reference the same instance.
    event_bus: Arc<AtpEventBus>,
    /// Proc handler retained so phase3 can wire in service_manager and tool_registry.
    proc_handler: Option<Arc<ProcHandler>>,
    /// Executor factory retained so phase3 can inject the real ToolRegistry.
    executor_factory: Option<Arc<executor_factory::IpcExecutorFactory>>,
    /// Trace flags set via `with_trace_flags()` before `start_daemon()`.
    trace_flags: TraceFlags,
    /// Active tracer — created at `start_daemon()` from `trace_flags`.
    tracer: Arc<Tracer>,
    /// Invocation store opened in phase2; retained for crash recovery in phase2.5.
    invocation_store: Option<Arc<InvocationStore>>,
    /// Session store opened in phase2; retained for crash recovery in phase2.5.
    session_store: Option<Arc<PersistentSessionStore>>,
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
        let service_pids = std::collections::HashMap::new();
        let vfs = Arc::new(VfsRouter::new());
        let process_table = Arc::new(ProcessTable::new());
        let runtime_dir = std::env::var("AVIX_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| root.join("run/avix"));
        let kernel_sock = std::env::var("AVIX_KERNEL_SOCK")
            .map(PathBuf::from)
            .unwrap_or_else(|_| runtime_dir.join("kernel.sock"));

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
            .map_err(|_| {
                AvixError::ConfigParse(
                    "etc/signing.key not found — run `avix server config init` first".into(),
                )
            })?
            .trim()
            .to_string();

        phase2::mount_persistent_trees(&vfs, root).await?;
        log.push(BootLogEntry {
            phase: BootPhase(2),
            message: "phase 2: config + master key + persistent mounts".into(),
        });

        log.push(BootLogEntry {
            phase: BootPhase(3),
            message: "phase 3: pending service start".into(),
        });

        Ok(Runtime {
            master_key: Arc::new(master_key),
            boot_log: log,
            service_pids,
            vfs,
            process_table,
            root: root.to_path_buf(),
            runtime_dir,
            kernel_sock,
            event_bus: Arc::new(AtpEventBus::default()),
            proc_handler: None,
            executor_factory: None,
            trace_flags: TraceFlags::default(),
            tracer: Tracer::noop(),
            invocation_store: None,
            session_store: None,
        })
    }

    /// Set trace flags before calling `start_daemon()`.
    pub fn with_trace_flags(mut self, flags: TraceFlags) -> Self {
        self.trace_flags = flags;
        self
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
        // Create tracer now that we know the log directory.
        let log_dir = self.root.join("logs");
        let tracer = Tracer::new(self.trace_flags.clone(), log_dir);
        self.tracer = tracer;

        // Phase 2: spawn kernel.agent PID1 (skip in test_mode — gateway uses TestIpcRouter)
        if !test_mode {
            self.phase2_kernel().await?;
        }
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(2),
            message: "phase 2: kernel.agent spawned".into(),
        });

        // Phase 2.5: crash recovery — fix stale Running/Paused records from prior run.
        // Must run before phase3 (services) and phase4 (ATP gateway) so no client ever
        // observes a Running/Paused record that has no live executor.
        if let (Some(inv_store), Some(sess_store)) =
            (self.invocation_store.clone(), self.session_store.clone())
        {
            phase3_crash_recovery(inv_store, sess_store).await?;
            self.boot_log.push(BootLogEntry {
                phase: BootPhase(2),
                message: "phase 2.5: crash recovery complete".into(),
            });
        }

        // Phase 3: spawn services
        self.phase3_services().await?;
        self.boot_log.push(BootLogEntry {
            phase: BootPhase(3),
            message: "phase 3: services spawned".into(),
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
        let master_key_bytes = hex::decode(&*self.master_key)
            .map_err(|e| AvixError::ConfigParse(format!("invalid master key: {}", e)))?;
        // VFS mounts /etc/avix → <root>/etc, so agents.yaml lives at <root>/etc/agents.yaml.
        let agents_yaml_path = self.root.join("etc/agents.yaml");

        // Open persistent stores — files are created under <root>/data/ if they don't exist yet.
        let invocation_store = Arc::new(
            InvocationStore::open(self.root.join("data/invocations.redb"))
                .await
                .map_err(|e| AvixError::ConfigParse(format!("open invocation store: {e}")))?
                .with_local(crate::memfs::local_provider::LocalProvider::new(
                    self.root.join("data/users"),
                )?),
        );
        let session_store = Arc::new(
            PersistentSessionStore::open(self.root.join("data/sessions.redb"))
                .await
                .map_err(|e| AvixError::ConfigParse(format!("open session store: {e}")))?,
        );

        // Shared in-process signal channel registry — wires SignalHandler → executor tasks.
        let signal_channels = SignalChannelRegistry::new();

        let factory = Arc::new(
            executor_factory::IpcExecutorFactory::new(
                Arc::clone(&self.process_table),
                Arc::clone(&self.event_bus),
                Arc::clone(&invocation_store),
                Arc::clone(&session_store),
            )
            .with_tracer(Arc::clone(&self.tracer))
            .with_signal_channels(signal_channels.clone()),
        );

        // Retain so phase3 can inject the real ToolRegistry.
        self.executor_factory = Some(Arc::clone(&factory));

        let scanner = Arc::new(ManifestScanner::new(Arc::clone(&self.vfs)));

        let proc_handler = Arc::new(
            ProcHandler::new_with_factory(
                Arc::clone(&self.process_table),
                agents_yaml_path,
                master_key_bytes,
                self.runtime_dir.clone(),
                factory,
            )
            .with_manifest_scanner(scanner)
            .with_tracer(Arc::clone(&self.tracer))
            .with_invocation_store(Arc::clone(&invocation_store))
            .with_session_store(Arc::clone(&session_store))
            .with_signal_channels(signal_channels),
        );
        // Retain references for crash recovery in phase 2.5.
        self.invocation_store = Some(Arc::clone(&invocation_store));
        self.session_store = Some(Arc::clone(&session_store));

        // Retain a reference so phase3 can wire in service_manager and tool_registry.
        self.proc_handler = Some(Arc::clone(&proc_handler));
        let kernel_server =
            KernelIpcServer::new(self.kernel_sock.clone(), proc_handler, self.root.clone());
        kernel_server.start().await?;
        tracing::info!(sock = %self.kernel_sock.display(), "kernel IPC server started");
        Ok(())
    }

    async fn phase3_services(&mut self) -> Result<(), AvixError> {
        // Shared registries used by the router and service manager.
        let (service_manager, tool_registry) =
            ServiceManager::new_with_registry(self.runtime_dir.clone());
        let service_manager = Arc::new(service_manager);
        let service_registry = Arc::new(ServiceRegistry::new());

        // Wire service_manager and tool_registry into the kernel IPC server so that
        // kernel/sys/service-list and kernel/sys/tool-list IPC methods are available.
        if let Some(ph) = &self.proc_handler {
            ph.set_service_manager(Arc::clone(&service_manager)).await;
            ph.set_tool_registry(Arc::clone(&tool_registry)).await;
        }

        // Inject real ToolRegistry into executor factory so spawned agents discover Cat1 tools.
        if let Some(factory) = &self.executor_factory {
            factory.set_tool_registry(Arc::clone(&tool_registry)).await;
        }

        // Register kernel syscalls in the tool registry for tool discovery via /tools/
        let syscall_reg = crate::syscall::SyscallRegistry::new();
        tool_registry.add_kernel_syscalls(&syscall_reg).await?;

        // Set tool registry on VFS router for /tools/ population
        self.vfs.set_tool_registry(Arc::clone(&tool_registry)).await;

        // Mount /tools/ VFS for tool discovery
        let tools_vfs = Arc::new(crate::memfs::vfs::MemFs::new());
        self.vfs.mount_memfs("/tools".to_string(), tools_vfs).await;
        tracing::debug!("/tools VFS mount registered (will be populated per-request)");

        // Bridge internal tool/service events to the ATP event bus for UI notifications.
        Arc::clone(&tool_registry)
            .start_atp_bridge(Arc::clone(&self.event_bus))
            .await;
        Arc::clone(&service_manager)
            .start_atp_bridge(Arc::clone(&self.event_bus))
            .await;

        // ── router.svc ────────────────────────────────────────────────────────
        let router_sock = self.runtime_dir.join("router.sock");
        let dispatcher = Arc::new(RouterDispatcher::new(
            Arc::clone(&service_registry),
            Arc::clone(&tool_registry),
            Arc::clone(&self.process_table),
        ));
        RouterIpcServer::new(router_sock.clone(), Arc::clone(&dispatcher))
            .start()
            .await?;
        tracing::info!(sock = %router_sock.display(), "router.svc started");

        // ── exec.svc ─────────────────────────────────────────────────────────
        let exec_sock = self.runtime_dir.join("exec.sock");
        ExecIpcServer::new(exec_sock.clone()).start().await?;
        tracing::info!(sock = %exec_sock.display(), "exec.svc started");

        // ── llm.svc (optional — requires etc/llm.yaml) ────────────────────────
        let llm_yaml_path = self.root.join("etc/llm.yaml");
        if llm_yaml_path.exists() {
            match std::fs::read_to_string(&llm_yaml_path)
                .map_err(|e| AvixError::ConfigParse(format!("failed to read etc/llm.yaml: {e}")))
                .and_then(|s| LlmConfig::from_str(&s))
            {
                Ok(llm_config) => {
                    let routing = Arc::new(RoutingEngine::from_config(&llm_config));
                    let llm_sock = self.runtime_dir.join("llm.sock");
                    match LlmIpcServer::new(
                        llm_sock.clone(),
                        llm_config,
                        HashMap::new(),
                        routing,
                        HashMap::new(),
                    )
                    .start()
                    .await
                    {
                        Ok(_handle) => {
                            tracing::info!(sock = %llm_sock.display(), "llm.svc started");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "llm.svc failed to start");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invalid etc/llm.yaml — llm.svc not started");
                }
            }
        } else {
            tracing::warn!("etc/llm.yaml not found — llm.svc not started");
        }

        // ── mcp-bridge.svc (optional — requires non-empty etc/mcp.json) ─────────
        let mcp_json_path = self.root.join("etc/mcp.json");
        if mcp_json_path.exists() {
            match McpConfig::load(&mcp_json_path) {
                Ok(cfg) if !cfg.mcp_servers.is_empty() => {
                    let mcp_sock = self.runtime_dir.join("mcp-bridge.sock");
                    let runner = McpBridgeRunner::new(
                        cfg,
                        self.kernel_sock.clone(),
                        "svc-token-mcp-bridge".to_string(),
                        mcp_sock.clone(),
                    );
                    match runner.start().await {
                        Ok(_bridge) => {
                            tracing::info!(sock = %mcp_sock.display(), "mcp-bridge.svc started");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "mcp-bridge.svc failed to start — continuing");
                        }
                    }
                }
                Ok(_) => {
                    tracing::debug!("mcp.json has no servers — mcp-bridge.svc not started");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to parse etc/mcp.json — mcp-bridge.svc not started");
                }
            }
        }

        // ── installed third-party services ───────────────────────────────────
        let watchdog_entries = Arc::new(tokio::sync::RwLock::new(HashMap::new()));

        match ServiceManager::discover_installed(&self.root) {
            Ok(units) => {
                for unit in units {
                    let token = match service_manager
                        .spawn_and_get_token(ServiceSpawnRequest::from_unit(&unit))
                        .await
                    {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(name = %unit.name, error = %e, "failed to allocate token for service");
                            continue;
                        }
                    };

                    match ServiceProcess::spawn(
                        &unit,
                        &token,
                        &self.kernel_sock,
                        &router_sock,
                        &self.runtime_dir,
                    )
                    .await
                    {
                        Ok(process) => {
                            tracing::info!(
                                name = %unit.name,
                                pid = token.pid.as_u64(),
                                "installed service started"
                            );
                            watchdog_entries.write().await.insert(
                                unit.name.clone(),
                                WatchdogEntry {
                                    unit,
                                    process,
                                    restart_count: 0,
                                },
                            );
                        }
                        Err(e) => {
                            tracing::warn!(name = %unit.name, error = %e, "failed to start service");
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "discover_installed failed — no third-party services started");
            }
        }

        // Start watchdog for installed services.
        // Dropping ServiceWatchdog detaches the handle; the tokio task continues running.
        let _watchdog = ServiceWatchdog::start(
            watchdog_entries,
            Arc::clone(&service_manager),
            self.kernel_sock.clone(),
            router_sock,
            self.runtime_dir.clone(),
        );

        tracing::info!("phase 3: built-in services started");
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
        let event_bus = Arc::clone(&self.event_bus);
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
        let server = GatewayServer::new(config, auth_svc, token_store, event_bus)
            .with_tracer(Arc::clone(&self.tracer));
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
