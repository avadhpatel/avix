use crate::error::AvixError;
use crate::params::resolver::{Annotations, ResolvedConfig};
use serde::{Deserialize, Serialize};
use tracing::instrument;

// ── Metadata ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFor {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMetadata {
    pub target: String,
    pub resolved_at: String,
    pub resolved_for: ResolvedFor,
    pub crews: Vec<String>,
}

// ── Full envelope (the file on disk) ─────────────────────────────────────────

/// The `kind: Resolved` YAML file written to `/proc/<pid>/resolved.yaml`.
///
/// The `resolved` block contains merged param values from the resolution engine.
/// The `granted_tools` field captures the capability token's tool list so that
/// consumers can read a single file for both config and capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: ResolvedMetadata,
    pub resolved: ResolvedConfig,
    /// Tools from the CapabilityToken — included for convenience.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_tools: Vec<String>,
    /// Provenance annotations — omitted from `/proc/<pid>/resolved.yaml` by default;
    /// included in `/proc/users/<u>/resolved/` preview files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

impl ResolvedFile {
    /// Build a new `ResolvedFile`.
    #[instrument]
    pub fn new(
        username: impl Into<String> + std::fmt::Debug,
        pid: Option<u64>,
        crews: Vec<String>,
        resolved: ResolvedConfig,
        granted_tools: Vec<String>,
        annotations: Option<Annotations>,
    ) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "Resolved".into(),
            metadata: ResolvedMetadata {
                target: "agent-manifest".into(),
                resolved_at: chrono::Utc::now().to_rfc3339(),
                resolved_for: ResolvedFor {
                    username: username.into(),
                    pid,
                },
                crews,
            },
            resolved,
            granted_tools,
            annotations,
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[instrument]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    #[instrument]
    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::defaults::system_agent_defaults;
    use crate::params::limits::system_agent_limits;
    use crate::params::resolver::{ParamResolver, ResolverInput};

    fn make_resolved() -> (ResolvedConfig, Annotations) {
        let input = ResolverInput {
            system_defaults: system_agent_defaults(),
            system_defaults_path: "/kernel/defaults/agent-manifest.yaml".into(),
            system_limits: system_agent_limits(),
            system_limits_path: "/kernel/limits/agent-manifest.yaml".into(),
            crew_defaults: vec![],
            crew_limits: vec![],
            user_defaults: None,
            user_limits: None,
            manifest: crate::params::defaults::AgentDefaults::default(),
        };
        ParamResolver::resolve(&input).unwrap()
    }

    #[test]
    fn resolved_file_round_trips() {
        let (cfg, annotations) = make_resolved();
        let file = ResolvedFile::new(
            "alice",
            Some(42),
            vec!["research".into()],
            cfg,
            vec!["fs/read".into(), "llm/complete".into()],
            Some(annotations),
        );
        let yaml = file.to_yaml().unwrap();
        let parsed = ResolvedFile::from_str(&yaml).unwrap();
        assert_eq!(parsed.metadata.resolved_for.username, "alice");
        assert_eq!(parsed.metadata.resolved_for.pid, Some(42));
        assert!(parsed.granted_tools.contains(&"fs/read".to_string()));
        assert!(parsed.annotations.is_some());
    }

    #[test]
    fn resolved_file_no_annotations_when_omitted() {
        let (cfg, _) = make_resolved();
        let file = ResolvedFile::new("bob", Some(7), vec![], cfg, vec![], None);
        let yaml = file.to_yaml().unwrap();
        assert!(!yaml.contains("annotations"));
    }

    #[test]
    fn resolved_file_contains_granted_tools() {
        let (cfg, _) = make_resolved();
        let tools = vec!["fs/read".into(), "fs/write".into()];
        let file = ResolvedFile::new("alice", Some(10), vec![], cfg, tools, None);
        let yaml = file.to_yaml().unwrap();
        assert!(
            yaml.contains("fs/read"),
            "resolved yaml must contain fs/read"
        );
        assert!(
            yaml.contains("fs/write"),
            "resolved yaml must contain fs/write"
        );
    }
}
