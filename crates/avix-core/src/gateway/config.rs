use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub user_addr: SocketAddr,
    pub admin_addr: SocketAddr,
    /// In dev/test mode, TLS is disabled.
    pub tls_enabled: bool,
    pub hil_timeout_secs: u64,
    /// Path to the kernel IPC socket. Falls back to `AVIX_KERNEL_SOCK` env var if None.
    pub kernel_sock: Option<PathBuf>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            user_addr: "127.0.0.1:7700".parse().unwrap(),
            admin_addr: "127.0.0.1:7701".parse().unwrap(),
            tls_enabled: false,
            hil_timeout_secs: 600,
            kernel_sock: None,
        }
    }
}
