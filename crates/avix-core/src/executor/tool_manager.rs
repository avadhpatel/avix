use crate::types::token::CapabilityToken;

use super::tool_registration::{cat2_tool_descriptor, compute_cat2_tools};
use super::validation::ToolBudgets;

/// Manages the category-2 tool list, budgets, and HIL gating for an agent session.
pub struct ToolManager {
    /// Current tool descriptors sent to the LLM each turn.
    pub tool_list: Vec<serde_json::Value>,
    /// Per-tool call budgets.
    pub tool_budgets: ToolBudgets,
    /// Tools that require HIL approval before dispatch.
    pub hil_required_tools: Vec<String>,
    /// Tools explicitly removed from the agent's view (via `handle_tool_changed("removed",...)`).
    pub removed_tools: Vec<String>,
    /// Cat2 tool names registered with the tool registry at spawn.
    pub registered_cat2: Vec<String>,
}

impl ToolManager {
    pub fn new(registered_cat2: Vec<String>) -> Self {
        Self {
            tool_list: Vec::new(),
            tool_budgets: ToolBudgets::default(),
            hil_required_tools: Vec::new(),
            removed_tools: Vec::new(),
            registered_cat2,
        }
    }

    /// Rebuild `tool_list` from current Cat2 tools, excluding removed tools.
    pub fn refresh_tool_list(&mut self, token: &CapabilityToken, spawned_by: &str) {
        let cat2 = compute_cat2_tools(token, spawned_by);
        let removed = &self.removed_tools;
        self.tool_list = cat2
            .into_iter()
            .filter(|(name, _)| !removed.contains(name))
            .map(|(name, _)| cat2_tool_descriptor(&name))
            .collect();
    }

    /// Return tool descriptors filtered to exclude removed tools.
    pub fn current_tool_list(&self) -> Vec<serde_json::Value> {
        self.tool_list
            .iter()
            .filter(|t| {
                if let Some(name) = t["name"].as_str() {
                    !self.removed_tools.iter().any(|r| {
                        let mangled = r.replace('/', "__");
                        name == r.as_str() || name == mangled.as_str()
                    })
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    }

    /// Returns true if this tool is a registered Category 2 tool.
    pub fn is_cat2_tool(&self, name: &str) -> bool {
        self.registered_cat2.contains(&name.to_string())
    }

    /// Handle a tool-changed notification (added/removed).
    pub fn handle_tool_changed(&mut self, op: &str, tool_name: &str) {
        match op {
            "removed" => {
                if !self.removed_tools.contains(&tool_name.to_string()) {
                    self.removed_tools.push(tool_name.to_string());
                }
            }
            "added" => {
                self.removed_tools.retain(|t| t != tool_name);
            }
            _ => {}
        }
    }

    /// Set a per-tool call budget.
    pub fn set_tool_budget(&mut self, tool: &str, n: u32) {
        self.tool_budgets.set(tool, n);
    }

    /// Register a tool that requires HIL approval before dispatch.
    pub fn require_hil_for(&mut self, tool: &str) {
        self.hil_required_tools.push(tool.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::CapabilityToken;

    fn make_manager(caps: &[&str]) -> ToolManager {
        let token = CapabilityToken::test_token(caps);
        let cat2 = compute_cat2_tools(&token, "kernel");
        let registered_cat2: Vec<String> = cat2.iter().map(|(n, _)| n.clone()).collect();
        let mut mgr = ToolManager::new(registered_cat2);
        mgr.refresh_tool_list(&token, "kernel");
        mgr
    }

    #[test]
    fn tool_list_populated() {
        let mgr = make_manager(&[]);
        assert!(!mgr.tool_list.is_empty(), "always-present tools must appear");
    }

    #[test]
    fn removed_tool_excluded() {
        let mut mgr = make_manager(&[]);
        mgr.handle_tool_changed("removed", "cap/list");
        let names: Vec<_> = mgr
            .current_tool_list()
            .into_iter()
            .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
            .collect();
        assert!(!names.contains(&"cap/list".to_string()));
    }

    #[test]
    fn added_tool_re_enabled() {
        let mut mgr = make_manager(&[]);
        mgr.handle_tool_changed("removed", "cap/list");
        mgr.handle_tool_changed("added", "cap/list");
        let names: Vec<_> = mgr
            .current_tool_list()
            .into_iter()
            .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
            .collect();
        assert!(names.contains(&"cap/list".to_string()));
    }

    #[test]
    fn is_cat2_tool_correct() {
        let mgr = make_manager(&["agent/spawn"]);
        assert!(mgr.is_cat2_tool("cap/list")); // always-present
        assert!(mgr.is_cat2_tool("agent/spawn"));
        assert!(!mgr.is_cat2_tool("fs/read"));
    }

    #[test]
    fn set_budget_and_remaining() {
        let mut mgr = make_manager(&["fs/read"]);
        mgr.set_tool_budget("fs/read", 3);
        assert_eq!(mgr.tool_budgets.remaining("fs/read"), Some(3));
    }

    #[test]
    fn require_hil_for_records_tool() {
        let mut mgr = make_manager(&[]);
        mgr.require_hil_for("cap/escalate");
        assert!(mgr.hil_required_tools.contains(&"cap/escalate".to_string()));
    }
}
