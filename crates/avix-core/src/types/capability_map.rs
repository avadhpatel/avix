use std::collections::HashMap;

/// Maps capability grant names to the Category 2 tools they unlock.
///
/// Capability names use `namespace:verb` format consistently:
///   "agent:spawn"     → agent orchestration tools
///   "pipe:use"        → inter-agent pipe tools
///   "llm:inference"   → llm/complete
///   "llm:image"       → llm/generate-image
///   etc.
///
/// NOTE: `granted_tools` in a CapabilityToken stores *individual tool names*
/// (e.g. "agent/spawn", "fs/read"), not capability group names. This map is used:
///   - By token issuers: tools_for_capability("agent:spawn") to know which tools to grant
///   - By compute_cat2_tools: all_gated_cat2_tools() to check which Cat2 tools are in a token
///
/// `job/watch` is always-present and does NOT require a separate capability grant.
pub struct CapabilityToolMap {
    map: HashMap<&'static str, Vec<&'static str>>,
    always: Vec<&'static str>,
}

impl Default for CapabilityToolMap {
    fn default() -> Self {
        let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        map.insert(
            "agent:spawn",
            vec![
                "agent/spawn",
                "agent/kill",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        );
        map.insert(
            "pipe:use",
            vec!["pipe/open", "pipe/write", "pipe/read", "pipe/close"],
        );
        map.insert("llm:inference", vec!["llm/complete"]);
        map.insert("llm:image", vec!["llm/generate-image"]);
        map.insert("llm:speech", vec!["llm/generate-speech"]);
        map.insert("llm:transcription", vec!["llm/transcribe"]);
        map.insert("llm:embedding", vec!["llm/embed"]);

        // Memory capability grants.
        // memory:write is a superset of memory:read — includes all read tools plus write tools.
        map.insert(
            "memory:read",
            vec!["memory/retrieve", "memory/get-fact", "memory/get-preferences"],
        );
        map.insert(
            "memory:write",
            vec![
                "memory/retrieve",
                "memory/get-fact",
                "memory/get-preferences",
                "memory/log-event",
                "memory/store-fact",
                "memory/update-preference",
                "memory/forget",
            ],
        );
        map.insert("memory:share", vec!["memory/share-request"]);

        Self {
            map,
            always: vec!["cap/request-tool", "cap/escalate", "cap/list", "job/watch"],
        }
    }
}

impl CapabilityToolMap {
    /// Returns all Cat2 tools granted by a specific capability name.
    /// Used by token issuers to expand a capability into individual tool grants.
    pub fn tools_for_capability(&self, cap: &str) -> &[&'static str] {
        self.map.get(cap).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Returns all Cat2 tools that require a capability grant (i.e. not always-present).
    /// Used by compute_cat2_tools to check which tools from the token are Cat2 tools.
    pub fn all_gated_cat2_tools(&self) -> Vec<&'static str> {
        self.map.values().flatten().copied().collect()
    }

    pub fn always_present(&self) -> &[&'static str] {
        &self.always
    }
}
