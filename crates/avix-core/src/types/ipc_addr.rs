use super::Pid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcAddr(String);

impl IpcAddr {
    pub fn from_name(runtime_dir: &std::path::Path, name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("{}/{}.sock", runtime_dir.display(), name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-{}", name));
    }

    pub fn for_agent(runtime_dir: &std::path::Path, pid: Pid) -> Self {
        #[cfg(unix)]
        return Self(format!("{}/agents/{}.sock", runtime_dir.display(), pid));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-agent-{}", pid));
    }

    pub fn for_service(runtime_dir: &std::path::Path, name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("{}/services/{}.sock", runtime_dir.display(), name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-svc-{}", name));
    }

    pub fn router(runtime_dir: &std::path::Path) -> Self {
        Self::from_name(runtime_dir, "router")
    }
    pub fn kernel(runtime_dir: &std::path::Path) -> Self {
        Self::from_name(runtime_dir, "kernel")
    }
    pub fn auth(runtime_dir: &std::path::Path) -> Self {
        Self::from_name(runtime_dir, "auth")
    }
    pub fn memfs(runtime_dir: &std::path::Path) -> Self {
        Self::from_name(runtime_dir, "memfs")
    }

    pub fn os_path(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_well_known_addresses() {
        let runtime_dir = Path::new("/run/avix");
        assert!(IpcAddr::kernel(runtime_dir).os_path().contains("kernel"));
        assert!(IpcAddr::auth(runtime_dir).os_path().contains("auth"));
        assert!(IpcAddr::memfs(runtime_dir).os_path().contains("memfs"));
        assert!(IpcAddr::router(runtime_dir).os_path().contains("router"));
    }
}
