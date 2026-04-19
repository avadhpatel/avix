use crate::types::pid::Pid;
use std::path::{Path, PathBuf};
use tracing::instrument;

/// Resolved OS path to the kernel socket.
/// Services read this from `AVIX_KERNEL_SOCK`; this function provides the default.
#[instrument]
pub fn kernel_sock_path(run_dir: &Path) -> PathBuf {
    run_dir.join("kernel.sock")
}

/// Resolved OS path to the router socket.
/// Services read this from `AVIX_ROUTER_SOCK`.
#[instrument]
pub fn router_sock_path(run_dir: &Path) -> PathBuf {
    run_dir.join("router.sock")
}

/// Resolved OS path for an agent's inbound signal socket.
/// Path: `<run_dir>/agents/<pid>.sock`
#[instrument]
pub fn agent_sock_path(run_dir: &Path, pid: Pid) -> PathBuf {
    run_dir
        .join("agents")
        .join(format!("{}.sock", pid.as_u64()))
}

/// Resolved OS path for a named service socket.
/// Path: `<run_dir>/services/<name>.sock`
#[instrument]
pub fn svc_sock_path(run_dir: &Path, name: &str) -> PathBuf {
    run_dir.join("services").join(format!("{name}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::pid::Pid;

    #[instrument]
    #[test]
    fn kernel_sock_path_is_under_run_dir() {
        let dir = std::path::Path::new("/run/avix");
        assert_eq!(
            kernel_sock_path(dir),
            PathBuf::from("/run/avix/kernel.sock")
        );
    }

    #[instrument]
    #[test]
    fn router_sock_path_is_under_run_dir() {
        let dir = std::path::Path::new("/run/avix");
        assert_eq!(
            router_sock_path(dir),
            PathBuf::from("/run/avix/router.sock")
        );
    }

    #[instrument]
    #[test]
    fn agent_sock_path_uses_pid() {
        let dir = std::path::Path::new("/run/avix");
        let pid = Pid::from_u64(57);
        assert_eq!(
            agent_sock_path(dir, pid),
            PathBuf::from("/run/avix/agents/57.sock")
        );
    }

    #[instrument]
    #[test]
    fn svc_sock_path_uses_name() {
        let dir = std::path::Path::new("/run/avix");
        assert_eq!(
            svc_sock_path(dir, "github-svc"),
            PathBuf::from("/run/avix/services/github-svc.sock")
        );
    }
}
