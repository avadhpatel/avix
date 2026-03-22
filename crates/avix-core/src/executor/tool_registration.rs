use crate::types::capability_map::CapabilityToolMap;
use crate::types::{token::CapabilityToken, tool::ToolVisibility};
use std::collections::HashSet;

pub fn compute_cat2_tools(token: &CapabilityToken) -> Vec<(String, ToolVisibility)> {
    let map = CapabilityToolMap::default();
    let mut tools = Vec::new();

    // Always-present tools
    for &name in map.always_present() {
        tools.push((name.to_string(), ToolVisibility::All));
    }

    // Capability-gated tools
    for cap in &token.granted_tools {
        for &name in map.tools_for_capability(cap) {
            tools.push((name.to_string(), ToolVisibility::All));
        }
    }

    // Deduplicate
    let mut seen = HashSet::new();
    tools.retain(|(name, _)| seen.insert(name.clone()));
    tools
}
