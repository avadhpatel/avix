use std::sync::Arc;

use crate::memfs::{VfsPath, VfsRouter};

use super::RuntimeExecutor;

impl RuntimeExecutor {
    /// Write `/proc/<pid>/status.yaml` and `/proc/<pid>/resolved.yaml` to the VFS.
    /// Must be called after `with_vfs()`. No-op when no VFS is attached.
    pub async fn init_proc_files(&self) {
        self.init_proc_files_for(&self.spawned_by.clone(), &[]).await;
    }

    /// Like `init_proc_files`, but with explicit username and crew memberships.
    pub async fn init_proc_files_for(&self, username: &str, crews: &[String]) {
        let vfs = match &self.vfs {
            Some(v) => Arc::clone(v),
            None => return,
        };
        self.write_status_yaml(&vfs).await;
        self.write_resolved_file(&vfs, username, crews).await;
    }

    /// Build and write `/proc/<pid>/status.yaml` from current executor state.
    pub(super) async fn write_status_yaml(&self, vfs: &VfsRouter) {
        use crate::process::entry::{ProcessEntry, ProcessKind, WaitingOn};
        use crate::process::status_file::AgentStatusFile;
        use std::sync::atomic::Ordering;

        let pid = self.pid.as_u64();

        let state = if self.killed.load(Ordering::Acquire) {
            crate::process::entry::ProcessStatus::Stopped
        } else if self.paused.load(Ordering::Acquire) {
            crate::process::entry::ProcessStatus::Paused
        } else {
            crate::process::entry::ProcessStatus::Running
        };

        let last_signal = self.last_signal_received.lock().await.clone();

        let entry = ProcessEntry {
            pid: self.pid,
            name: self.agent_name.clone(),
            kind: ProcessKind::Agent,
            status: state,
            spawned_by_user: self.spawned_by.clone(),
            goal: self.goal.clone(),
            spawned_at: self.spawned_at,
            context_used: self.context_used,
            context_limit: self.context_limit,
            last_activity_at: chrono::Utc::now(),
            waiting_on: None::<WaitingOn>,
            granted_tools: self.token.granted_tools.clone(),
            denied_tools: self.denied_tools.clone(),
            tool_chain_depth: 0,
            tokens_consumed: self.tokens_consumed,
            tool_calls_total: self.tool_calls_total,
            last_signal_received: last_signal,
            pending_signal_count: self.pending_signal_count.load(Ordering::Relaxed),
            ..ProcessEntry::default()
        };

        let file = AgentStatusFile::from_entry(&entry, vec![]);
        match file.to_yaml() {
            Ok(yaml) => {
                if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/status.yaml")) {
                    let _ = vfs.write(&path, yaml).await;
                }
            }
            Err(e) => {
                tracing::warn!(pid, "failed to serialise status.yaml: {e}");
            }
        }
    }

    pub(super) async fn write_resolved_file(
        &self,
        vfs: &VfsRouter,
        username: &str,
        crews: &[String],
    ) {
        use crate::params::defaults::system_agent_defaults;
        use crate::params::limits::system_agent_limits;
        use crate::params::resolved_file::ResolvedFile;
        use crate::params::resolver::{ParamResolver, ResolverInput, ResolverInputLoader};

        let pid = self.pid.as_u64();

        let loader = ResolverInputLoader::new(vfs);
        let mut input = match loader.load(username, crews).await {
            Ok(inp) => inp,
            Err(_) => ResolverInput {
                system_defaults: system_agent_defaults(),
                system_defaults_path: "compiled-in".into(),
                system_limits: system_agent_limits(),
                system_limits_path: "compiled-in".into(),
                crew_defaults: vec![],
                crew_limits: vec![],
                user_defaults: None,
                user_limits: None,
                manifest: crate::params::defaults::AgentDefaults::default(),
            },
        };
        input.manifest = crate::params::defaults::AgentDefaults::default();

        let (resolved_config, _annotations) = match ParamResolver::resolve(&input) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("param resolution failed for pid {pid}: {e}");
                return;
            }
        };

        let file = ResolvedFile::new(
            username,
            Some(pid),
            crews.to_vec(),
            resolved_config,
            self.token.granted_tools.clone(),
            None,
        );

        match file.to_yaml() {
            Ok(yaml) => {
                if let Ok(path) = VfsPath::parse(&format!("/proc/{pid}/resolved.yaml")) {
                    let _ = vfs.write(&path, yaml.into_bytes()).await;
                }
            }
            Err(e) => {
                tracing::warn!("failed to serialise resolved.yaml for pid {pid}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
    use crate::executor::SpawnParams;
    use crate::memfs::VfsRouter;
    use crate::types::{token::CapabilityToken, Pid};

    async fn make_executor_with_vfs(pid_val: u64) -> (RuntimeExecutor, Arc<VfsRouter>) {
        let registry = Arc::new(MockToolRegistry::new());
        let params = SpawnParams {
            pid: Pid::from_u64(pid_val),
            agent_name: "proc-test-agent".into(),
            goal: "proc test".into(),
            spawned_by: "kernel".into(),
            session_id: "sess-proc".into(),
            token: CapabilityToken::test_token(&[]),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
            invocation_id: String::new(),
            atp_session_id: String::new(),
        };
        let vfs = Arc::new(VfsRouter::new());
        let executor = RuntimeExecutor::spawn_with_registry(params, registry)
            .await
            .unwrap()
            .with_vfs(Arc::clone(&vfs));
        (executor, vfs)
    }

    #[tokio::test]
    async fn init_proc_files_writes_status_yaml() {
        let (executor, vfs) = make_executor_with_vfs(900).await;
        executor.init_proc_files().await;
        let path = crate::memfs::VfsPath::parse("/proc/900/status.yaml").unwrap();
        let bytes = vfs.read(&path).await.unwrap();
        let yaml = String::from_utf8(bytes).unwrap();
        assert!(yaml.contains("pid: 900") || yaml.contains("name: proc-test-agent"));
    }

    #[tokio::test]
    async fn init_proc_files_no_vfs_no_panic() {
        let registry = Arc::new(MockToolRegistry::new());
        let params = SpawnParams {
            pid: Pid::from_u64(901),
            agent_name: "no-vfs".into(),
            goal: "g".into(),
            spawned_by: "kernel".into(),
            session_id: "s".into(),
            token: CapabilityToken::test_token(&[]),
            system_prompt: None,
            selected_model: "claude-sonnet-4".into(),
            denied_tools: vec![],
            context_limit: 0,
            runtime_dir: std::path::PathBuf::new(),
            invocation_id: String::new(),
            atp_session_id: String::new(),
        };
        let executor = RuntimeExecutor::spawn_with_registry(params, registry)
            .await
            .unwrap();
        // No VFS — must not panic
        executor.init_proc_files().await;
    }
}
