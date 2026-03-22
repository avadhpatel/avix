use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum ATPCommand {
    AgentSpawn { name: String, goal: String },
    AgentKill { pid: u32 },
    AgentList,
    AgentStatus { pid: u32 },
    FsRead { path: String },
    FsWrite { path: String, content: String },
    LlmStatus,
    SysInfo,
    SysReboot { confirm: bool },
}

impl ATPCommand {
    pub fn from_json(value: &Value) -> Option<Self> {
        serde_json::from_value(value.clone()).ok()
    }
}
