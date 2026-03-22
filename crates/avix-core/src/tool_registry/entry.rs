use crate::types::tool::{ToolName, ToolState, ToolVisibility};

#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: ToolName,
    pub owner: String,
    pub state: ToolState,
    pub visibility: ToolVisibility,
    pub descriptor: serde_json::Value,
}
