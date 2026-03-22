use super::Role;
use serde::{Deserialize, Serialize};

/// Session-level token (used in auth::session)
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub role: Role,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    pub granted_tools: Vec<String>,
    pub signature: String,
}

impl CapabilityToken {
    pub fn has_tool(&self, tool: &str) -> bool {
        self.granted_tools.iter().any(|t| t == tool)
    }
}
