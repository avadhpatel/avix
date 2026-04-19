#[allow(clippy::too_many_arguments)]
use tracing::instrument;

#[instrument(skip(tool_budgets, pending_messages, tools, denied_tools))]
pub fn build_system_prompt(
    pid: u64,
    agent_name: &str,
    goal: &str,
    spawned_by: &str,
    session_id: &str,
    max_tool_chain_length: usize,
    tool_budgets: &std::collections::HashMap<String, u32>,
    pending_messages: &[String],
    tools: &[serde_json::Value],
    denied_tools: &[String],
) -> String {
    // Block 1 — Agent Identity (static, set at spawn)
    let mut prompt = format!(
        "# Agent Identity\nYou are {agent_name}, an AI agent running inside Avix.\nYour goal: {goal}\nSession: {session_id} | PID: {pid} | User: {spawned_by}\n"
    );

    // Block 2 — Available Tools (dynamic, rebuilt on tool.changed events)
    // Lists every tool currently available to this agent. Rebuilt whenever a
    // tool.changed event fires so the LLM never attempts a currently unavailable tool.
    prompt.push_str("\n# Available Tools\n");
    if tools.is_empty() {
        prompt.push_str("No tools currently available.\n");
    } else {
        for tool in tools {
            let name = tool["name"].as_str().unwrap_or("?");
            let desc = tool["description"].as_str().unwrap_or("");
            prompt.push_str(&format!("- **{name}**: {desc}\n"));
        }
    }
    prompt.push_str(
        "\nWhen you need a tool not listed here, call cap/request-tool.\n\
         When you encounter a situation requiring human judgment, call cap/escalate.\n\
         When your task is complete, respond with your final answer.\n",
    );

    // Block 3 — Constraints (static, set at spawn)
    let has_budgets = !tool_budgets.is_empty();
    let nontrivial = max_tool_chain_length != usize::MAX || has_budgets;
    if nontrivial {
        prompt.push_str(&format!(
            "\n# Constraints\nMax tool calls per turn: {max_tool_chain_length}\n"
        ));
        if has_budgets {
            prompt.push_str("Tool call budgets:\n");
            let mut entries: Vec<_> = tool_budgets.iter().collect();
            entries.sort_by_key(|(k, _)| k.as_str());
            for (tool, n) in entries {
                prompt.push_str(&format!("  {tool}: {n} use(s) remaining\n"));
            }
        }
    }

    // Block 4 — Pending Instructions (dynamic, injected by RuntimeExecutor)
    // Used for: HIL escalation guidance, HIL denial feedback,
    // tool availability changes, memory summaries.
    let has_pending = !pending_messages.is_empty();
    let has_denied = !denied_tools.is_empty();
    if has_pending || has_denied {
        prompt.push_str("\n# Pending Instructions\n");
        for msg in pending_messages {
            prompt.push_str(msg);
            prompt.push('\n');
        }
        if has_denied {
            let list = denied_tools.join(", ");
            prompt.push_str(&format!(
                "**Tool access denied this turn:** {list}\n\
                 Do not call cap/request-tool for these tools again until the next user message.\n"
            ));
        }
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_budgets(pairs: &[(&str, u32)]) -> HashMap<String, u32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn make_tools(names: &[&str]) -> Vec<serde_json::Value> {
        names
            .iter()
            .map(|n| json!({"name": n, "description": format!("Description of {n}")}))
            .collect()
    }

    #[test]
    fn test_prompt_contains_all_blocks() {
        let budgets = make_budgets(&[("send_email", 1)]);
        let tools = make_tools(&["fs/read", "cap/escalate"]);
        let prompt = build_system_prompt(
            42,
            "my-agent",
            "do the thing",
            "alice",
            "sess-abc",
            10,
            &budgets,
            &["Some pending instruction.".to_string()],
            &tools,
            &[],
        );
        assert!(prompt.contains("# Agent Identity"), "missing block 1");
        assert!(prompt.contains("# Available Tools"), "missing block 2");
        assert!(prompt.contains("# Constraints"), "missing block 3");
        assert!(prompt.contains("# Pending Instructions"), "missing block 4");
    }

    #[test]
    fn test_prompt_block2_lists_tools() {
        let tools = make_tools(&["fs/read", "cap/list"]);
        let prompt = build_system_prompt(
            1,
            "agent",
            "goal",
            "user",
            "sess-1",
            usize::MAX,
            &HashMap::new(),
            &[],
            &tools,
            &[],
        );
        assert!(prompt.contains("**fs/read**"));
        assert!(prompt.contains("**cap/list**"));
    }

    #[test]
    fn test_prompt_block2_empty_tools() {
        let prompt = build_system_prompt(
            1,
            "agent",
            "goal",
            "user",
            "sess-1",
            usize::MAX,
            &HashMap::new(),
            &[],
            &[],
            &[],
        );
        assert!(prompt.contains("No tools currently available."));
    }

    #[test]
    fn test_prompt_block3_with_budgets() {
        let budgets = make_budgets(&[("fs/write", 3), ("send_email", 1)]);
        let prompt =
            build_system_prompt(1, "agent", "goal", "user", "sess-1", 5, &budgets, &[], &[], &[]);
        assert!(prompt.contains("fs/write: 3 use(s) remaining"));
        assert!(prompt.contains("send_email: 1 use(s) remaining"));
    }

    #[test]
    fn test_prompt_no_pending_skips_block4() {
        let prompt = build_system_prompt(
            1,
            "agent",
            "goal",
            "user",
            "sess-1",
            10,
            &HashMap::new(),
            &[],
            &[],
            &[],
        );
        assert!(!prompt.contains("# Pending Instructions"));
    }

    #[test]
    fn test_prompt_denied_tools_block4() {
        let denied = vec!["fs/write".to_string(), "send_email".to_string()];
        let prompt = build_system_prompt(
            1,
            "agent",
            "goal",
            "user",
            "sess-1",
            10,
            &HashMap::new(),
            &[],
            &[],
            &denied,
        );
        assert!(prompt.contains("# Pending Instructions"));
        assert!(prompt.contains("**Tool access denied this turn:** fs/write, send_email"));
        assert!(prompt.contains("Do not call cap/request-tool for these tools again"));
    }

    #[test]
    fn test_prompt_denied_tools_empty_no_warning() {
        let prompt = build_system_prompt(
            1,
            "agent",
            "goal",
            "user",
            "sess-1",
            10,
            &HashMap::new(),
            &[],
            &[],
            &[],
        );
        assert!(!prompt.contains("Tool access denied"));
    }
}
