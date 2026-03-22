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
            "description": "Spawn a child agent to work on a sub-task. Requires spawn capability.",
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
    fn test_cat2_descriptor_all_tools() {
        let known = [
            "cap/request-tool",
            "cap/escalate",
            "cap/list",
            "job/watch",
            "agent/spawn",
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
}
