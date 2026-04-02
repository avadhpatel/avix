use crate::types::tool::{ToolName, ToolState, ToolVisibility};

#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: ToolName,
    pub owner: String,
    pub state: ToolState,
    pub visibility: ToolVisibility,
    pub descriptor: serde_json::Value,
    pub capabilities_required: Vec<String>,
}

impl ToolEntry {
    pub fn new(
        name: ToolName,
        owner: String,
        state: ToolState,
        visibility: ToolVisibility,
        descriptor: serde_json::Value,
    ) -> Self {
        Self {
            name,
            owner,
            state,
            visibility,
            descriptor,
            capabilities_required: Vec::new(),
        }
    }

    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities_required = capabilities;
        self
    }
}
