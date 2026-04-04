use super::descriptor::SyscallDescriptor;
use std::collections::HashMap;

pub struct SyscallRegistry {
    syscalls: Vec<SyscallDescriptor>,
    by_name: HashMap<String, usize>,
}

impl SyscallRegistry {
    pub fn new() -> Self {
        let syscalls = vec![
            SyscallDescriptor::new(
                "kernel/proc/spawn",
                "proc",
                "Spawn a new agent process",
                "Creates a new agent process with the given manifest.\n\nPermissions: caller must have `agent:spawn` capability",
                vec!["agent:spawn"],
                "fn proc_::spawn(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/proc/kill",
                "proc",
                "Terminate an agent process",
                "Terminates a running agent process by PID.\n\nPermissions: caller must have `agent:kill` capability",
                vec!["agent:kill"],
                "fn proc_::kill(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/proc/list",
                "proc",
                "List running processes",
                "Returns list of all running agent processes.\n\nPermissions: caller must have `agent:list` capability",
                vec!["agent:list"],
                "fn proc_::list(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/proc/info",
                "proc",
                "Get process info",
                "Returns detailed information about a specific process.\n\nPermissions: caller must have `agent:info` capability",
                vec!["agent:info"],
                "fn proc_::info(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/proc/wait",
                "proc",
                "Wait for process exit",
                "Blocks until the specified process exits.\n\nPermissions: caller must have `agent:wait` capability",
                vec!["agent:wait"],
                "fn proc_::wait(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/proc/signal",
                "proc",
                "Send signal to process",
                "Sends a signal (SIGKILL, SIGPAUSE, etc.) to a process.\n\nPermissions: caller must have `agent:signal` capability",
                vec!["agent:signal"],
                "fn proc_::signal(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/read",
                "fs",
                "Read file from VFS",
                "Reads bytes from a VFS path.\n\nPermissions: caller must have `fs/read` capability",
                vec!["fs:read"],
                "fn fs_::read(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/write",
                "fs",
                "Write file to VFS",
                "Writes bytes to a VFS path.\n\nPermissions: caller must have `fs:write` capability",
                vec!["fs:write"],
                "fn fs_::write(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/list",
                "fs",
                "List directory contents",
                "Lists entries in a VFS directory.\n\nPermissions: caller must have `fs:list` capability",
                vec!["fs:list"],
                "fn fs_::list(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/exists",
                "fs",
                "Check if path exists",
                "Checks if a VFS path exists.\n\nPermissions: caller must have `fs:read` capability",
                vec!["fs:read"],
                "fn fs_::exists(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/delete",
                "fs",
                "Delete file or directory",
                "Deletes a file or directory from VFS.\n\nPermissions: caller must have `fs:delete` capability",
                vec!["fs:delete"],
                "fn fs_::delete(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/fs/watch",
                "fs",
                "Watch for file changes",
                "Sets up a watch on a VFS path for change notifications.\n\nPermissions: caller must have `fs:watch` capability",
                vec!["fs:watch"],
                "fn fs_::watch(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/cap/issue",
                "cap",
                "Issue capability token",
                "Issues a new capability token to another process.\n\nPermissions: caller must have `cap:issue` capability",
                vec!["cap:issue"],
                "fn cap_::issue(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/cap/validate",
                "cap",
                "Validate capability token",
                "Validates a capability token's signature and grants.\n\nPermissions: caller must have `cap:validate` capability",
                vec!["cap:validate"],
                "fn cap_::validate(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/cap/revoke",
                "cap",
                "Revoke capability token",
                "Revokes a previously issued capability token.\n\nPermissions: caller must have `cap:revoke` capability",
                vec!["cap:revoke"],
                "fn cap_::revoke(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/cap/policy",
                "cap",
                "Manage capability policy",
                "Gets or sets the system-wide capability policy.\n\nPermissions: caller must have `cap:policy` capability",
                vec!["cap:policy"],
                "fn cap_::policy(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sys/info",
                "sys",
                "Get system information",
                "Returns system-wide information (version, uptime, etc.).\n\nPermissions: none required",
                vec![],
                "fn sys_::info(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sys/boot-log",
                "sys",
                "Get boot log",
                "Returns the system boot log entries.\n\nPermissions: admin only",
                vec!["admin:boot-log"],
                "fn sys_::boot_log(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sys/reboot",
                "sys",
                "Reboot the system",
                "Initiates a system reboot.\n\nPermissions: admin only, requires confirm=true",
                vec!["admin:reboot"],
                "fn sys_::reboot(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sched/cron-add",
                "sched",
                "Add cron job",
                "Adds a new scheduled cron job.\n\nPermissions: caller must have `sched:cron` capability",
                vec!["sched:cron"],
                "fn sched_::cron_add(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sched/cron-remove",
                "sched",
                "Remove cron job",
                "Removes a scheduled cron job.\n\nPermissions: caller must have `sched:cron` capability",
                vec!["sched:cron"],
                "fn sched_::cron_remove(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/sched/cron-list",
                "sched",
                "List cron jobs",
                "Lists all scheduled cron jobs.\n\nPermissions: caller must have `sched:cron` capability",
                vec!["sched:cron"],
                "fn sched_::cron_list(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/snap/save",
                "snap",
                "Save process snapshot",
                "Saves a snapshot of a process's state.\n\nPermissions: caller must have `snap:save` capability",
                vec!["snap:save"],
                "fn snap_::save(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/snap/restore",
                "snap",
                "Restore process snapshot",
                "Restores a process from a saved snapshot.\n\nPermissions: caller must have `snap:restore` capability",
                vec!["snap:restore"],
                "fn snap_::restore(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/snap/list",
                "snap",
                "List process snapshots",
                "Lists all snapshots for a process.\n\nPermissions: caller must have `snap:list` capability",
                vec!["snap:list"],
                "fn snap_::list(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "kernel/snap/delete",
                "snap",
                "Delete process snapshot",
                "Deletes a saved snapshot.\n\nPermissions: caller must have `snap:delete` capability",
                vec!["snap:delete"],
                "fn snap_::delete(ctx: &SyscallContext, params: Value) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "proc/package/install-agent",
                "package",
                "Install agent from package",
                "Installs an agent pack from a remote URL or local file.\n\nPermissions: caller must have `proc/package/install-agent` capability. For non-official sources, also requires `install:from-untrusted-source`.",
                vec!["proc/package/install-agent"],
                "fn pkg_::install_agent(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "proc/package/install-service",
                "package",
                "Install service from package",
                "Installs a service pack from a remote URL or local file.\n\nPermissions: caller must have `proc/package/install-service` capability. For non-official sources, also requires `install:from-untrusted-source`.",
                vec!["proc/package/install-service"],
                "fn pkg_::install_service(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "proc/package/uninstall-agent",
                "package",
                "Uninstall agent",
                "Uninstalls an installed agent pack.\n\nPermissions: caller must have `proc/package/install-agent` capability.",
                vec!["proc/package/install-agent"],
                "fn pkg_::uninstall_agent(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult"
            ),
            SyscallDescriptor::new(
                "proc/package/uninstall-service",
                "package",
                "Uninstall service",
                "Uninstalls an installed service pack.\n\nPermissions: caller must have `proc/package/install-service` capability.",
                vec!["proc/package/install-service"],
                "fn pkg_::uninstall_service(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult"
            ),
        ];

        let mut by_name = HashMap::new();
        for (i, syscall) in syscalls.iter().enumerate() {
            by_name.insert(syscall.name.clone(), i);
        }

        Self { syscalls, by_name }
    }

    pub fn list(&self) -> &[SyscallDescriptor] {
        &self.syscalls
    }

    pub fn get(&self, name: &str) -> Option<&SyscallDescriptor> {
        self.by_name.get(name).map(|&i| &self.syscalls[i])
    }

    pub fn list_by_domain(&self, domain: &str) -> Vec<&SyscallDescriptor> {
        self.syscalls
            .iter()
            .filter(|s| s.domain == domain)
            .collect()
    }

    pub fn domains(&self) -> Vec<&str> {
        let mut domains: Vec<&str> = self.syscalls.iter().map(|s| s.domain.as_str()).collect();
        domains.sort();
        domains.dedup();
        domains
    }
}

impl Default for SyscallRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syscall_registry_contains_all_syscalls() {
        let reg = SyscallRegistry::new();
        let list = reg.list();
        assert!(
            list.len() >= 24,
            "Expected at least 24 syscalls, got {}",
            list.len()
        );
    }

    #[test]
    fn test_syscall_registry_lookups() {
        let reg = SyscallRegistry::new();
        assert!(reg.get("kernel/proc/spawn").is_some());
        assert!(reg.get("kernel/fs/read").is_some());
        assert!(reg.get("kernel/cap/issue").is_some());
        assert!(reg.get("kernel/nonexistent").is_none());
    }

    #[test]
    fn test_syscall_registry_list_by_domain() {
        let reg = SyscallRegistry::new();
        let proc_syscalls = reg.list_by_domain("proc");
        assert!(!proc_syscalls.is_empty());
        assert!(proc_syscalls
            .iter()
            .all(|s| s.name.starts_with("kernel/proc/")));
    }

    #[test]
    fn test_syscall_registry_domains() {
        let reg = SyscallRegistry::new();
        let domains = reg.domains();
        assert!(domains.contains(&"proc"));
        assert!(domains.contains(&"fs"));
        assert!(domains.contains(&"cap"));
        assert!(domains.contains(&"sys"));
        assert!(domains.contains(&"sched"));
        assert!(domains.contains(&"snap"));
    }
}
