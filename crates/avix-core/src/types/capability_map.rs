use std::collections::HashMap;

pub struct CapabilityToolMap {
    map: HashMap<&'static str, Vec<&'static str>>,
    always: Vec<&'static str>,
}

impl Default for CapabilityToolMap {
    fn default() -> Self {
        let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        map.insert(
            "spawn",
            vec![
                "agent/spawn",
                "agent/list",
                "agent/wait",
                "agent/send-message",
            ],
        );
        map.insert(
            "pipe",
            vec!["pipe/open", "pipe/write", "pipe/read", "pipe/close"],
        );
        map.insert("llm:inference", vec!["llm/complete"]);
        map.insert("llm:image", vec!["llm/generate-image"]);
        map.insert("llm:speech", vec!["llm/generate-speech"]);
        map.insert("llm:transcription", vec!["llm/transcribe"]);
        map.insert("llm:embedding", vec!["llm/embed"]);

        Self {
            map,
            always: vec!["cap/request-tool", "cap/escalate", "cap/list", "job/watch"],
        }
    }
}

impl CapabilityToolMap {
    pub fn tools_for_capability(&self, cap: &str) -> &[&'static str] {
        let base = cap.split("::").next().unwrap_or(cap);
        self.map.get(base).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn always_present(&self) -> &[&'static str] {
        &self.always
    }
}
