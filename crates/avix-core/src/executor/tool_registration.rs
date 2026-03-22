use crate::types::capability_map::CapabilityToolMap;
use crate::types::{token::CapabilityToken, tool::ToolVisibility};
use std::collections::HashSet;

/// Compute the Category 2 tool set for an agent given its token and owning username.
///
/// `CapabilityToken.granted_tools` stores **individual tool names** (e.g. "agent/spawn",
/// "fs/read"). This function:
///   1. Always includes the 4 always-present tools (cap/*, job/watch) — no token check.
///   2. Scans all known Cat2 gated tools; includes each one that appears in the token.
///
/// All Cat2 tools are scoped to the agent's owning user (ToolVisibility::User).
pub fn compute_cat2_tools(
    token: &CapabilityToken,
    username: &str,
) -> Vec<(String, ToolVisibility)> {
    let map = CapabilityToolMap::default();
    let mut tools = Vec::new();

    // Always-present tools (registered regardless of token contents)
    for &name in map.always_present() {
        tools.push((name.to_string(), ToolVisibility::User(username.to_string())));
    }

    // Capability-gated Cat2 tools: register only those explicitly in the token
    for name in map.all_gated_cat2_tools() {
        if token.has_tool(name) {
            tools.push((name.to_string(), ToolVisibility::User(username.to_string())));
        }
    }

    // Deduplicate (always-present tools may also appear in the gated list)
    let mut seen = HashSet::new();
    tools.retain(|(name, _)| seen.insert(name.clone()));
    tools
}

/// Return a JSON tool descriptor in Avix-native format for a Category 2 tool name.
pub fn cat2_tool_descriptor(name: &str) -> serde_json::Value {
    match name {
        "cap/request-tool" => serde_json::json!({
            "name": "cap/request-tool",
            "description": "Request access to a tool not currently in the agent's CapabilityToken. This triggers a HIL capability_upgrade event.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "tool":    { "type": "string", "description": "Name of the tool to request" },
                    "reason":  { "type": "string", "description": "Why the agent needs this tool" },
                    "urgency": { "type": "string", "description": "Urgency level: low | medium | high" }
                },
                "required": ["tool", "reason", "urgency"]
            }
        }),
        "cap/escalate" => serde_json::json!({
            "name": "cap/escalate",
            "description": "Proactively ask a human for guidance when the agent is uncertain how to proceed.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "reason":  { "type": "string",  "description": "Situation description" },
                    "context": { "type": "string",  "description": "Relevant context" },
                    "options": { "type": "array",   "description": "List of options for the human to choose from" }
                },
                "required": ["reason", "context", "options"]
            }
        }),
        "cap/list" => serde_json::json!({
            "name": "cap/list",
            "description": "List the agent's currently granted tools and constraints.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        "job/watch" => serde_json::json!({
            "name": "job/watch",
            "description": "Subscribe to events from a long-running job and block until completion.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "jobId":      { "type": "string", "description": "The job ID to watch" },
                    "timeoutSec": { "type": "number", "description": "Seconds to wait before timing out" }
                },
                "required": ["jobId", "timeoutSec"]
            }
        }),
        "agent/spawn" => serde_json::json!({
            "name": "agent/spawn",
            "description": "Spawn a child agent to work on a sub-task. Requires agent:spawn capability.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "agent":         { "type": "string",  "description": "Agent name (must exist in /bin/)" },
                    "goal":          { "type": "string",  "description": "Goal for the child agent" },
                    "capabilities":  { "type": "array",   "description": "Requested capabilities (subset of parent's grants)" },
                    "waitForResult": { "type": "boolean", "description": "If true, block until child finishes" }
                },
                "required": ["agent", "goal", "capabilities", "waitForResult"]
            }
        }),
        "agent/kill" => serde_json::json!({
            "name": "agent/kill",
            "description": "Terminate a child agent by PID. Requires agent:spawn capability.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pid":    { "type": "number", "description": "PID of the child agent to terminate" },
                    "reason": { "type": "string", "description": "Reason for termination" }
                },
                "required": ["pid"]
            }
        }),
        "agent/list" => serde_json::json!({
            "name": "agent/list",
            "description": "List agents currently running in this session, optionally filtered by status.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "Optional filter: running | paused | waiting | all" }
                },
                "required": []
            }
        }),
        "agent/wait" => serde_json::json!({
            "name": "agent/wait",
            "description": "Block until a specific child agent completes.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pid":        { "type": "number", "description": "PID of the child agent to wait for" },
                    "timeoutSec": { "type": "number", "description": "Optional timeout in seconds (0 = wait forever)" }
                },
                "required": ["pid"]
            }
        }),
        "agent/send-message" => serde_json::json!({
            "name": "agent/send-message",
            "description": "Send a message to another agent via its input pipe.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pid":     { "type": "number", "description": "Target agent PID" },
                    "message": { "type": "string", "description": "Message to send" }
                },
                "required": ["pid", "message"]
            }
        }),
        "pipe/open" => serde_json::json!({
            "name": "pipe/open",
            "description": "Open a streaming data channel to another agent.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "targetPid":    { "type": "number", "description": "PID of the target agent" },
                    "direction":    { "type": "string", "description": "out | bidirectional" },
                    "bufferTokens": { "type": "number", "description": "Optional buffer size in tokens" },
                    "backpressure": { "type": "string", "description": "Optional: block | drop | error" }
                },
                "required": ["targetPid", "direction"]
            }
        }),
        "pipe/write" => serde_json::json!({
            "name": "pipe/write",
            "description": "Write tokens into an open outbound pipe.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pipeId":  { "type": "string", "description": "The pipe identifier" },
                    "content": { "type": "string", "description": "Content to write into the pipe" }
                },
                "required": ["pipeId", "content"]
            }
        }),
        "pipe/read" => serde_json::json!({
            "name": "pipe/read",
            "description": "Read tokens from an open inbound pipe.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pipeId":    { "type": "string", "description": "The pipe identifier" },
                    "maxTokens": { "type": "number", "description": "Optional maximum tokens to read" },
                    "timeoutMs": { "type": "number", "description": "Optional timeout in milliseconds (0 = block indefinitely)" }
                },
                "required": ["pipeId"]
            }
        }),
        "pipe/close" => serde_json::json!({
            "name": "pipe/close",
            "description": "Close a pipe. The other end receives SIGPIPE.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pipeId": { "type": "string", "description": "The pipe identifier" }
                },
                "required": ["pipeId"]
            }
        }),
        other => serde_json::json!({
            "name": other,
            "description": "",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::CapabilityToken;

    fn token_with_tools(tools: &[&str]) -> CapabilityToken {
        CapabilityToken::test_token(tools)
    }

    #[test]
    fn test_cat2_descriptor_cap_list() {
        let desc = cat2_tool_descriptor("cap/list");
        assert_eq!(desc["name"], "cap/list");
    }

    #[test]
    fn test_cat2_descriptor_agent_spawn() {
        let desc = cat2_tool_descriptor("agent/spawn");
        assert!(!desc["description"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn test_cat2_descriptor_agent_kill() {
        let desc = cat2_tool_descriptor("agent/kill");
        assert_eq!(desc["name"], "agent/kill");
        assert!(!desc["description"].as_str().unwrap_or("").is_empty());
    }

    #[test]
    fn test_cat2_descriptor_all_tools() {
        let known = [
            "cap/request-tool",
            "cap/escalate",
            "cap/list",
            "job/watch",
            "agent/spawn",
            "agent/kill",
            "agent/list",
            "agent/wait",
            "agent/send-message",
            "pipe/open",
            "pipe/write",
            "pipe/read",
            "pipe/close",
        ];
        for name in &known {
            let desc = cat2_tool_descriptor(name);
            let got_name = desc["name"].as_str().unwrap_or("");
            assert!(!got_name.is_empty(), "descriptor for {name} has empty name");
        }
    }

    #[test]
    fn test_compute_cat2_tools_always_present() {
        // Empty token → only always-present tools
        let token = token_with_tools(&[]);
        let tools = compute_cat2_tools(&token, "alice");
        let names: Vec<_> = tools.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"cap/request-tool"));
        assert!(names.contains(&"cap/escalate"));
        assert!(names.contains(&"cap/list"));
        assert!(names.contains(&"job/watch"));
        // No gated Cat2 tools without explicit grants
        assert!(!names.contains(&"agent/spawn"));
        assert!(!names.contains(&"pipe/open"));
    }

    #[test]
    fn test_compute_cat2_tools_individual_agent_tools() {
        // Token holds individual tool names — only listed tools are registered
        let token = token_with_tools(&["agent/spawn", "agent/kill"]);
        let tools = compute_cat2_tools(&token, "alice");
        let names: Vec<_> = tools.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"agent/spawn"));
        assert!(names.contains(&"agent/kill"));
        // agent/list not granted → not registered
        assert!(!names.contains(&"agent/list"));
    }

    #[test]
    fn test_compute_cat2_tools_pipe_tools() {
        let token = token_with_tools(&["pipe/open", "pipe/write", "pipe/read", "pipe/close"]);
        let tools = compute_cat2_tools(&token, "alice");
        let names: Vec<_> = tools.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"pipe/open"));
        assert!(names.contains(&"pipe/write"));
        assert!(names.contains(&"pipe/read"));
        assert!(names.contains(&"pipe/close"));
    }

    #[test]
    fn test_compute_cat2_tools_user_visibility() {
        let token = token_with_tools(&["agent/spawn"]);
        let tools = compute_cat2_tools(&token, "bob");
        for (_, vis) in &tools {
            assert_eq!(
                *vis,
                crate::types::tool::ToolVisibility::User("bob".to_string())
            );
        }
    }

    #[test]
    fn test_compute_cat2_tools_no_cat1_tools_registered() {
        // Cat1 tools in token (fs/read) should NOT appear in Cat2 registration
        let token = token_with_tools(&["fs/read", "agent/spawn"]);
        let tools = compute_cat2_tools(&token, "alice");
        let names: Vec<_> = tools.iter().map(|(n, _)| n.as_str()).collect();
        assert!(!names.contains(&"fs/read"));
        assert!(names.contains(&"agent/spawn"));
    }
}
