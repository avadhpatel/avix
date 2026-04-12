use serde_json::{json, Value};
use thiserror::Error;

use crate::types::role::Role;

use super::atp::ATPCommand;

#[derive(Debug, Error)]
pub enum TranslationError {
    #[error("EPERM: role {0:?} cannot execute {1}")]
    Eperm(Role, String),
    #[error("unknown command")]
    UnknownCommand,
}

pub struct IpcCall {
    pub method: String,
    pub params: Value,
}

pub struct ATPTranslator;

impl ATPTranslator {
    pub fn translate(
        &self,
        cmd: &ATPCommand,
        caller_role: &Role,
    ) -> Result<IpcCall, TranslationError> {
        match cmd {
            ATPCommand::AgentSpawn { name, goal } => Ok(IpcCall {
                method: "kernel/proc/spawn".into(),
                params: json!({ "name": name, "goal": goal }),
            }),
            ATPCommand::AgentKill { pid } => {
                let pid_u64: u64 = pid.parse().unwrap_or(0);
                Ok(IpcCall {
                    method: "kernel/proc/kill".into(),
                    params: json!({ "pid": pid_u64 }),
                })
            }
            ATPCommand::AgentList => Ok(IpcCall {
                method: "kernel/proc/list".into(),
                params: json!({}),
            }),
            ATPCommand::AgentStatus { pid } => {
                let pid_u64: u64 = pid.parse().unwrap_or(0);
                Ok(IpcCall {
                    method: "kernel/proc/info".into(),
                    params: json!({ "pid": pid_u64 }),
                })
            }
            ATPCommand::FsRead { path } => Ok(IpcCall {
                method: "kernel/fs/read".into(),
                params: json!({ "path": path }),
            }),
            ATPCommand::FsWrite { path, content } => Ok(IpcCall {
                method: "kernel/fs/write".into(),
                params: json!({ "path": path, "content": content }),
            }),
            ATPCommand::LlmStatus => Ok(IpcCall {
                method: "llm/status".into(),
                params: json!({}),
            }),
            ATPCommand::SysInfo => Ok(IpcCall {
                method: "kernel/sys/info".into(),
                params: json!({}),
            }),
            ATPCommand::SysReboot { confirm } => {
                // Only admin can reboot
                if *caller_role != Role::Admin {
                    return Err(TranslationError::Eperm(*caller_role, "sys/reboot".into()));
                }
                Ok(IpcCall {
                    method: "kernel/sys/reboot".into(),
                    params: json!({ "confirm": confirm }),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::{ATPCommand, ATPResponse};
    use serde_json::json;

    #[test]
    fn test_agent_spawn_command_parses() {
        let v = json!({
            "method": "agent_spawn",
            "params": { "name": "test", "goal": "do stuff" }
        });
        let cmd = ATPCommand::from_json(&v);
        assert!(cmd.is_some());
    }

    #[test]
    fn test_unknown_command_returns_none() {
        let v = json!({ "method": "unknown_xyz", "params": {} });
        let cmd = ATPCommand::from_json(&v);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_response_ok_shape() {
        let resp = ATPResponse::ok("req-1", json!({ "pid": 100 }));
        assert!(resp.ok);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_response_error_shape() {
        let resp = ATPResponse::err("req-1", "not found");
        assert!(!resp.ok);
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_agent_spawn_translates_to_proc_spawn() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::AgentSpawn {
            name: "x".into(),
            goal: "y".into(),
        };
        let call = translator.translate(&cmd, &Role::Admin).unwrap();
        assert_eq!(call.method, "kernel/proc/spawn");
    }

    #[test]
    fn test_fs_read_translates_correctly() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::FsRead {
            path: "/users/alice/data".into(),
        };
        let call = translator.translate(&cmd, &Role::Admin).unwrap();
        assert_eq!(call.method, "kernel/fs/read");
    }

    #[test]
    fn test_non_admin_cannot_reboot() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::SysReboot { confirm: true };
        let res = translator.translate(&cmd, &Role::Guest);
        assert!(matches!(res, Err(TranslationError::Eperm(_, _))));
    }

    #[test]
    fn test_admin_can_reboot() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::SysReboot { confirm: true };
        let res = translator.translate(&cmd, &Role::Admin);
        assert!(res.is_ok());
    }

    #[test]
    fn test_agent_kill_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::AgentKill { pid: "42".to_string() };
        let call = translator.translate(&cmd, &Role::Admin).unwrap();
        assert_eq!(call.method, "kernel/proc/kill");
        assert_eq!(call.params["pid"], 42u64);
    }

    #[test]
    fn test_agent_list_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::AgentList;
        let call = translator.translate(&cmd, &Role::User).unwrap();
        assert_eq!(call.method, "kernel/proc/list");
    }

    #[test]
    fn test_agent_status_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::AgentStatus { pid: "7".to_string() };
        let call = translator.translate(&cmd, &Role::User).unwrap();
        assert_eq!(call.method, "kernel/proc/info");
        assert_eq!(call.params["pid"], 7u64);
    }

    #[test]
    fn test_fs_write_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::FsWrite {
            path: "/users/alice/file.txt".into(),
            content: "hello".into(),
        };
        let call = translator.translate(&cmd, &Role::User).unwrap();
        assert_eq!(call.method, "kernel/fs/write");
        assert_eq!(call.params["content"], "hello");
    }

    #[test]
    fn test_llm_status_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::LlmStatus;
        let call = translator.translate(&cmd, &Role::User).unwrap();
        assert_eq!(call.method, "llm/status");
    }

    #[test]
    fn test_sys_info_translates() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::SysInfo;
        let call = translator.translate(&cmd, &Role::User).unwrap();
        assert_eq!(call.method, "kernel/sys/info");
    }

    #[test]
    fn test_response_serializes_to_json() {
        let resp = ATPResponse::ok("req-1", json!({ "status": "ok" }));
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"ok\":true"));
    }

    #[test]
    fn test_command_serializes_to_json() {
        let cmd = ATPCommand::AgentSpawn {
            name: "test".into(),
            goal: "do stuff".into(),
        };
        let serialized = serde_json::to_string(&cmd).unwrap();
        assert!(serialized.contains("agent_spawn"));
    }

    #[test]
    fn test_operator_cannot_reboot() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::SysReboot { confirm: true };
        let res = translator.translate(&cmd, &Role::Operator);
        assert!(matches!(res, Err(TranslationError::Eperm(_, _))));
    }

    #[test]
    fn test_user_cannot_reboot() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::SysReboot { confirm: true };
        let res = translator.translate(&cmd, &Role::User);
        assert!(matches!(res, Err(TranslationError::Eperm(_, _))));
    }

    #[test]
    fn test_response_id_preserved() {
        let resp = ATPResponse::ok("my-request-id", json!({}));
        assert_eq!(resp.id, "my-request-id");
    }

    #[test]
    fn test_spawn_params_contain_name_and_goal() {
        let translator = ATPTranslator;
        let cmd = ATPCommand::AgentSpawn {
            name: "my-agent".into(),
            goal: "my-goal".into(),
        };
        let call = translator.translate(&cmd, &Role::Admin).unwrap();
        assert_eq!(call.params["name"], "my-agent");
        assert_eq!(call.params["goal"], "my-goal");
    }
}
