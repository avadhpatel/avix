use crate::error::AvixError;
use crate::llm_svc::adapter::AvixToolCall;
use crate::types::token::CapabilityToken;
use std::collections::HashMap;

#[derive(Default)]
pub struct ToolBudgets {
    budgets: HashMap<String, u32>,
}

impl ToolBudgets {
    pub fn set(&mut self, tool: &str, budget: u32) {
        self.budgets.insert(tool.to_string(), budget);
    }

    pub fn remaining(&self, tool: &str) -> Option<u32> {
        self.budgets.get(tool).copied()
    }
}

pub fn validate_tool_call(
    token: &CapabilityToken,
    call: &AvixToolCall,
    budgets: &ToolBudgets,
) -> Result<(), AvixError> {
    if !token.granted_tools.is_empty() && !token.has_tool(&call.name) {
        return Err(AvixError::CapabilityDenied(format!(
            "Tool not granted: {}",
            call.name
        )));
    }
    if let Some(remaining) = budgets.remaining(&call.name) {
        if remaining == 0 {
            return Err(AvixError::CapabilityDenied(format!(
                "budget exhausted for tool: {}",
                call.name
            )));
        }
    }
    Ok(())
}
