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
///
/// Order of checks:
/// 1. Token expiry — an expired token blocks all tool calls without exception.
/// 2. Capability grant — always-present tools bypass this check (Invariant 13).
/// 3. Budget — if a per-tool budget is set, it must be > 0. Decremented on success.
pub fn validate_tool_call(
    token: &CapabilityToken,
    call: &AvixToolCall,
    budgets: &mut ToolBudgets,
) -> Result<(), AvixError> {
    // Expiry check — blocks everything, including always-present tools
    if token.is_expired() {
        return Err(AvixError::CapabilityDenied(
            "capability token has expired".into(),
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_call(name: &str) -> AvixToolCall {
        AvixToolCall {
            call_id: "test-id".into(),
            name: name.to_string(),
            args: serde_json::json!({}),
        }
    }

    #[test]
    fn expired_token_blocks_granted_tool() {
        let mut token = CapabilityToken::test_token(&["fs/read"]);
        token.expires_at = Utc::now() - chrono::Duration::seconds(1);
        let mut budgets = ToolBudgets::default();
        let err = validate_tool_call(&token, &make_call("fs/read"), &mut budgets).unwrap_err();
        assert!(
            err.to_string().contains("expired"),
            "expired token should be rejected: {err}"
        );
    }

    #[test]
    fn expired_token_blocks_always_present_tools() {
        let mut token = CapabilityToken::test_token(&[]);
        token.expires_at = Utc::now() - chrono::Duration::seconds(1);
        let mut budgets = ToolBudgets::default();
        // Even always-present tools are blocked by expiry
        let err =
            validate_tool_call(&token, &make_call("cap/list"), &mut budgets).unwrap_err();
        assert!(
            err.to_string().contains("expired"),
            "expired token should block always-present tools too: {err}"
        );
    }

    #[test]
    fn fresh_token_allows_granted_tool() {
        let token = CapabilityToken::test_token(&["fs/read"]);
        let mut budgets = ToolBudgets::default();
        assert!(validate_tool_call(&token, &make_call("fs/read"), &mut budgets).is_ok());
    }

    #[test]
    fn fresh_token_blocks_ungranted_tool() {
        let token = CapabilityToken::test_token(&["fs/read"]);
        let mut budgets = ToolBudgets::default();
        let err = validate_tool_call(&token, &make_call("fs/write"), &mut budgets).unwrap_err();
        assert!(err.to_string().contains("not granted"));
    }

    #[test]
    fn always_present_tools_bypass_capability_check_when_fresh() {
        let token = CapabilityToken::test_token(&["fs/read"]); // no cap/list granted
        for tool in ALWAYS_PRESENT {
            let mut budgets = ToolBudgets::default();
            assert!(
                validate_tool_call(&token, &make_call(tool), &mut budgets).is_ok(),
                "always-present tool {tool} should not be blocked by capability check"
            );
        }
    }

    #[test]
    fn budget_exhausted_blocks_call() {
        let token = CapabilityToken::test_token(&["send_email"]);
        let mut budgets = ToolBudgets::default();
        budgets.set("send_email", 1);
        // First call succeeds and decrements
        assert!(validate_tool_call(&token, &make_call("send_email"), &mut budgets).is_ok());
        // Second call fails — budget exhausted
        let err =
            validate_tool_call(&token, &make_call("send_email"), &mut budgets).unwrap_err();
        assert!(err.to_string().contains("budget exhausted"));
    }
}
