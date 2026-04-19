use crate::error::AvixError;
use tracing::instrument;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VfsPath(String);

impl VfsPath {
    #[instrument]
    pub fn parse(s: &str) -> Result<Self, AvixError> {
        if !s.starts_with('/') {
            return Err(AvixError::ConfigParse(format!(
                "VFS path must be absolute: '{s}'"
            )));
        }
        if s.contains("..") {
            return Err(AvixError::ConfigParse(format!(
                "VFS path must not contain '..': '{s}'"
            )));
        }
        Ok(Self(s.to_string()))
    }

    #[instrument]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[instrument]
    pub fn parent(&self) -> Option<VfsPath> {
        let (s, _) = self.0.rsplit_once('/')?;
        if s.is_empty() {
            Some(VfsPath("/".to_string()))
        } else {
            Some(VfsPath(s.to_string()))
        }
    }

    #[instrument]
    pub fn file_name(&self) -> Option<&str> {
        self.0.rsplit_once('/').map(|(_, name)| name)
    }

    /// Returns `true` if an agent (non-kernel caller) may write to this path.
    ///
    /// The following trees are kernel-owned and must never be written by agents:
    ///   `/proc/`      — kernel-generated runtime state
    ///   `/kernel/`    — compiled-in defaults and dynamic limits
    ///   `/secrets/`   — kernel-managed encrypted store
    ///   `/etc/avix/`  — system configuration (operator-only)
    ///   `/bin/`       — system agents (operator-only)
    ///
    /// Memory trees are also blocked — agents must use `memory.svc` tools instead
    /// of calling `fs/write` directly:
    ///   `/users/<user>/memory/`  — user agent memory (read/write via memory.svc)
    ///   `/crews/<crew>/memory/`  — crew shared memory (read/write via memory.svc)
    #[instrument]
    pub fn is_agent_writable(&self) -> bool {
        let p = self.as_str();
        // Kernel-owned trees
        if p.starts_with("/proc/")
            || p.starts_with("/kernel/")
            || p.starts_with("/secrets/")
            || p.starts_with("/etc/avix/")
            || p.starts_with("/bin/")
        {
            return false;
        }
        // Memory trees: agents may not call fs/write directly.
        // All memory writes go through memory.svc tools.
        if p.starts_with("/users/") && p.contains("/memory/") {
            return false;
        }
        if p.starts_with("/crews/") && p.contains("/memory/") {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parent_of_root_child_is_root() {
        let p = VfsPath::parse("/foo").unwrap();
        assert_eq!(p.parent().unwrap().as_str(), "/");
    }

    #[test]
    fn test_file_name() {
        let p = VfsPath::parse("/foo/bar.txt").unwrap();
        assert_eq!(p.file_name(), Some("bar.txt"));
    }
}
