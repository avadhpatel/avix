use crate::error::AvixError;
use crate::llm_svc::adapter::AvixToolCall;
use crate::types::token::CapabilityToken;
use std::collections::HashMap;

/// Always-present Cat2 tools that bypass the capability grant check (Architecture Invariant 13).
const ALWAYS_PRESENT: &[&str] = &["cap/request-tool", "cap/escalate", "cap/list", "job/watch"];

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

    pub fn decrement(&mut self, tool: &str) {
        if let Some(b) = self.budgets.get_mut(tool) {
            *b = b.saturating_sub(1);
        }
    }
}

/// Validate a tool call against the agent's capability token and budget.
/// Decrements the budget on success (spec §Budget Enforcement: "atomically decrements").
/// Always-present tools (cap/request-tool, cap/escalate, cap/list, job/watch) bypass
/// the capability grant check per Architecture Invariant 13.
pub fn validate_tool_call(
    token: &CapabilityToken,
    call: &AvixToolCall,
    budgets: &mut ToolBudgets,
) -> Result<(), AvixError> {
    // Capability check — skip for always-present tools
    if !token.granted_tools.is_empty()
        && !token.has_tool(&call.name)
        && !ALWAYS_PRESENT.contains(&call.name.as_str())
    {
        return Err(AvixError::CapabilityDenied(format!(
            "Tool not granted: {}",
            call.name
        )));
    }

    // Budget check
    if let Some(remaining) = budgets.remaining(&call.name) {
        if remaining == 0 {
            return Err(AvixError::CapabilityDenied(format!(
                "budget exhausted for tool: {}",
                call.name
            )));
        }
        budgets.decrement(&call.name);
    }

    Ok(())
}
