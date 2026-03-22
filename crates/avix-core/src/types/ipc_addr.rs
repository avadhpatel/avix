use super::Pid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcAddr(String);

impl IpcAddr {
    pub fn from_name(name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/{}.sock", name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-{}", name));
    }

    pub fn for_agent(pid: Pid) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/agents/{}.sock", pid));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-agent-{}", pid));
    }

    pub fn for_service(name: &str) -> Self {
        #[cfg(unix)]
        return Self(format!("/run/avix/services/{}.sock", name));
        #[cfg(windows)]
        return Self(format!(r"\\.\pipe\avix-svc-{}", name));
    }

    pub fn router() -> Self {
        Self::from_name("router")
    }
    pub fn kernel() -> Self {
        Self::from_name("kernel")
    }
    pub fn auth() -> Self {
        Self::from_name("auth")
    }
    pub fn memfs() -> Self {
        Self::from_name("memfs")
    }

    pub fn os_path(&self) -> &str {
        &self.0
    }
}
