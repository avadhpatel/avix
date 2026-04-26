use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::types::Pid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HilType {
    ToolCallApproval,
    CapabilityUpgrade,
    Escalation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HilState {
    Pending,
    Approved,
    Denied,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HilUrgency {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilRequest {
    pub api_version: String,
    pub kind: String,
    pub hil_id: String,
    pub pid: Pid,
    pub agent_name: String,
    pub hil_type: HilType,
    pub tool: Option<String>,
    pub args: Option<serde_json::Value>,
    pub reason: Option<String>,
    pub context: Option<String>,
    pub options: Option<Vec<HilOption>>,
    pub urgency: HilUrgency,
    pub approval_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: HilState,
    /// ATP connection session ID — used to route events to the right subscriber.
    /// Distinct from the agent's internal `session_id`.
    pub atp_session_id: String,
}

impl HilRequest {
    /// The VFS path where this request is written.
    #[instrument]
    pub fn vfs_path(&self) -> String {
        format!("/proc/{}/hil-queue/{}.yaml", self.pid, self.hil_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_request() -> HilRequest {
        HilRequest {
            api_version: "avix/v1".into(),
            kind: "HilRequest".into(),
            hil_id: "hil-abc".into(),
            pid: Pid::from_u64(57),
            agent_name: "researcher".into(),
            hil_type: HilType::ToolCallApproval,
            tool: Some("send_email".into()),
            args: None,
            reason: Some("wants to send email".into()),
            context: None,
            options: None,
            urgency: HilUrgency::Normal,
            approval_token: "tok".into(),
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            state: HilState::Pending,
            atp_session_id: "atp-sess-1".into(),
        }
    }

    #[test]
    fn vfs_path_format_is_correct() {
        let req = sample_request();
        assert_eq!(req.vfs_path(), "/proc/57/hil-queue/hil-abc.yaml");
    }

    #[test]
    fn hil_request_serialises_to_yaml() {
        let req = sample_request();
        let yaml = serde_yaml::to_string(&req).unwrap();
        assert!(yaml.contains("api_version"));
        assert!(yaml.contains("avix/v1"));
        assert!(yaml.contains("tool_call_approval"));
    }

    #[test]
    fn hil_state_roundtrips() {
        for state in [
            HilState::Pending,
            HilState::Approved,
            HilState::Denied,
            HilState::Timeout,
        ] {
            let s = serde_json::to_string(&state).unwrap();
            let back: HilState = serde_json::from_str(&s).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn hil_type_roundtrips() {
        for t in [
            HilType::ToolCallApproval,
            HilType::CapabilityUpgrade,
            HilType::Escalation,
        ] {
            let s = serde_json::to_string(&t).unwrap();
            let back: HilType = serde_json::from_str(&s).unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn hil_urgency_serialises_lowercase() {
        let s = serde_json::to_string(&HilUrgency::Normal).unwrap();
        assert_eq!(s, "\"normal\"");
        let h = serde_json::to_string(&HilUrgency::High).unwrap();
        assert_eq!(h, "\"high\"");
    }
}
