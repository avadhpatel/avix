use std::path::{Path, PathBuf};

use tokio::process::{Child, Command};

use crate::error::AvixError;
use crate::service::token::ServiceToken;
use crate::service::yaml::ServiceUnit;
use crate::types::Pid;

pub struct ServiceProcess {
    pub name: String,
    pub pid: Pid,
    pub child: Child,
    pub socket_path: PathBuf,
}

impl ServiceProcess {
    /// Resolve the IPC socket path for this service.
    pub fn socket_path_for(run_dir: &Path, name: &str, pid: Pid) -> PathBuf {
        #[cfg(unix)]
        {
            run_dir.join(format!("{name}-{}.sock", pid.as_u64()))
        }
        #[cfg(windows)]
        {
            PathBuf::from(format!(r"\\.\pipe\avix-svc-{name}-{}", pid.as_u64()))
        }
    }

    /// Spawn the service binary described by `unit` with the token env vars injected.
    pub async fn spawn(
        unit: &ServiceUnit,
        token: &ServiceToken,
        kernel_sock: &Path,
        router_sock: &Path,
        run_dir: &Path,
    ) -> Result<Self, AvixError> {
        let pid = token.pid;
        let socket_path = Self::socket_path_for(run_dir, &unit.name, pid);

        let mut cmd = Command::new(&unit.service.binary);
        cmd.envs(build_env(
            unit,
            token,
            kernel_sock,
            router_sock,
            &socket_path,
        ));
        #[cfg(unix)]
        cmd.process_group(0);

        let child = cmd.spawn().map_err(|e| {
            AvixError::ConfigParse(format!("failed to spawn {}: {e}", unit.service.binary))
        })?;

        Ok(Self {
            name: unit.name.clone(),
            pid,
            child,
            socket_path,
        })
    }

    /// True if the child process is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

pub(crate) fn build_env(
    _unit: &ServiceUnit,
    token: &ServiceToken,
    kernel_sock: &Path,
    router_sock: &Path,
    svc_sock: &Path,
) -> Vec<(String, String)> {
    vec![
        ("AVIX_KERNEL_SOCK".into(), kernel_sock.display().to_string()),
        ("AVIX_ROUTER_SOCK".into(), router_sock.display().to_string()),
        ("AVIX_SVC_SOCK".into(), svc_sock.display().to_string()),
        ("AVIX_SVC_TOKEN".into(), token.token_str.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_unit(name: &str) -> ServiceUnit {
        use crate::service::yaml::{ServiceSection, ToolsSection, UnitSection};
        ServiceUnit {
            name: name.into(),
            version: "1.0.0".into(),
            source: crate::service::yaml::ServiceSource::User,
            signature: None,
            unit: UnitSection::default(),
            service: ServiceSection {
                binary: "/bin/echo".into(),
                language: "any".into(),
                restart: Default::default(),
                restart_delay: "5s".into(),
                max_concurrent: 20,
                queue_max: 100,
                queue_timeout: "5s".into(),
                run_as: Default::default(),
            },
            capabilities: Default::default(),
            tools: ToolsSection {
                namespace: format!("/tools/{name}/"),
                provides: vec![],
            },
            jobs: Default::default(),
        }
    }

    #[test]
    fn socket_path_contains_name_and_pid() {
        let run_dir = Path::new("/run/avix");
        let path = ServiceProcess::socket_path_for(run_dir, "github-svc", Pid::from_u64(42));
        let s = path.to_string_lossy();
        assert!(s.contains("github-svc"));
        assert!(s.contains("42"));
    }

    #[test]
    fn build_env_contains_all_required_vars() {
        let unit = make_test_unit("echo-svc");
        let token = ServiceToken {
            token_str: "tok-123".into(),
            service_name: "echo-svc".into(),
            pid: Pid::from_u64(5),
        };
        let env = build_env(
            &unit,
            &token,
            Path::new("/run/avix/kernel.sock"),
            Path::new("/run/avix/router.sock"),
            Path::new("/run/avix/echo-5.sock"),
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AVIX_KERNEL_SOCK"));
        assert!(keys.contains(&"AVIX_ROUTER_SOCK"));
        assert!(keys.contains(&"AVIX_SVC_SOCK"));
        assert!(keys.contains(&"AVIX_SVC_TOKEN"));
        let tok = env.iter().find(|(k, _)| k == "AVIX_SVC_TOKEN").unwrap();
        assert_eq!(tok.1, "tok-123");
    }
}
