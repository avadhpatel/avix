use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::Pid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Starting,
    Running,
    Degraded,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: String,
    pub version: String,
    pub pid: Pid,
    pub state: ServiceState,
    pub endpoint: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub registered_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub restart_count: u32,
    pub tools: Vec<String>,
}

impl ServiceStatus {
    /// VFS path for this service's status file.
    pub fn vfs_path(name: &str) -> String {
        format!("/proc/services/{name}/status.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_path_format() {
        assert_eq!(
            ServiceStatus::vfs_path("github-svc"),
            "/proc/services/github-svc/status.yaml"
        );
    }

    #[test]
    fn service_status_serialises() {
        let status = ServiceStatus {
            name: "test-svc".into(),
            version: "1.0.0".into(),
            pid: Pid::from_u64(42),
            state: ServiceState::Running,
            endpoint: Some("/run/avix/test-svc-42.sock".into()),
            started_at: None,
            registered_at: None,
            stopped_at: None,
            restart_count: 0,
            tools: vec!["test/echo".into()],
        };
        let yaml = serde_yaml::to_string(&status).unwrap();
        assert!(yaml.contains("running"));
        assert!(yaml.contains("test-svc"));
    }

    #[test]
    fn service_state_variants_serialise() {
        let cases = [
            (ServiceState::Starting, "starting"),
            (ServiceState::Running, "running"),
            (ServiceState::Degraded, "degraded"),
            (ServiceState::Stopping, "stopping"),
            (ServiceState::Stopped, "stopped"),
            (ServiceState::Failed, "failed"),
        ];
        for (state, expected) in cases {
            let yaml = serde_yaml::to_string(&state).unwrap();
            assert!(yaml.trim() == expected, "expected {expected}, got {yaml}");
        }
    }
}
