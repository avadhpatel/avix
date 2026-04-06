use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AvixError;

// ── Entrypoint ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum EntrypointType {
    #[default]
    LlmLoop,
}

fn default_min_context_window() -> u32 {
    8_000
}
fn default_required_capabilities() -> Vec<String> {
    vec!["tool_use".into()]
}
fn default_recommended_model() -> String {
    "claude-sonnet-4".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequirements {
    #[serde(default = "default_min_context_window")]
    pub min_context_window: u32,
    #[serde(default = "default_required_capabilities")]
    pub required_capabilities: Vec<String>,
    #[serde(default = "default_recommended_model")]
    pub recommended: String,
}

impl Default for ModelRequirements {
    fn default() -> Self {
        Self {
            min_context_window: default_min_context_window(),
            required_capabilities: default_required_capabilities(),
            recommended: default_recommended_model(),
        }
    }
}

fn default_max_tool_chain() -> u32 {
    5
}
fn default_max_turns_per_goal() -> u32 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntrypoint {
    #[serde(rename = "type", default)]
    pub entrypoint_type: EntrypointType,
    #[serde(default)]
    pub model_requirements: ModelRequirements,
    #[serde(default = "default_max_tool_chain")]
    pub max_tool_chain: u32,
    #[serde(default = "default_max_turns_per_goal")]
    pub max_turns_per_goal: u32,
}

impl Default for ManifestEntrypoint {
    fn default() -> Self {
        Self {
            entrypoint_type: EntrypointType::LlmLoop,
            model_requirements: ModelRequirements::default(),
            max_tool_chain: default_max_tool_chain(),
            max_turns_per_goal: default_max_turns_per_goal(),
        }
    }
}

// ── Tools ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestTools {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

// ── Memory ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkingContextMode {
    #[default]
    Dynamic,
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticStoreAccess {
    #[default]
    None,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMemory {
    #[serde(default)]
    pub working_context: WorkingContextMode,
    #[serde(default)]
    pub episodic_persistence: bool,
    #[serde(default)]
    pub semantic_store_access: SemanticStoreAccess,
}

// ── Snapshot ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotMode {
    #[default]
    Disabled,
    PerTurn,
}

fn default_compression_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSnapshot {
    #[serde(default)]
    pub mode: SnapshotMode,
    #[serde(default)]
    pub restore_on_crash: bool,
    #[serde(default = "default_compression_enabled")]
    pub compression_enabled: bool,
}

impl Default for ManifestSnapshot {
    fn default() -> Self {
        Self {
            mode: SnapshotMode::Disabled,
            restore_on_crash: false,
            compression_enabled: true,
        }
    }
}

// ── Defaults / Environment ────────────────────────────────────────────────────

fn default_temperature() -> f32 {
    0.7
}
fn default_top_p() -> f32 {
    0.9
}
fn default_timeout_sec() -> u32 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEnvironment {
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u32,
}

impl Default for ManifestEnvironment {
    fn default() -> Self {
        Self {
            temperature: default_temperature(),
            top_p: default_top_p(),
            timeout_sec: default_timeout_sec(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_template: Option<String>,
    #[serde(default)]
    pub environment: ManifestEnvironment,
}

// ── Shared metadata ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMetadata {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackagingMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

// ── Spec ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_path: Option<String>,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    #[serde(default)]
    pub entrypoint: ManifestEntrypoint,
    #[serde(default)]
    pub tools: ManifestTools,
    #[serde(default)]
    pub memory: ManifestMemory,
    #[serde(default)]
    pub snapshot: ManifestSnapshot,
    #[serde(default)]
    pub defaults: ManifestDefaults,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

// ── Top-level document ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: ManifestMetadata,
    #[serde(default)]
    pub packaging: PackagingMetadata,
    pub spec: AgentSpec,
}

impl AgentManifest {
    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// VFS path for a system-installed agent: `/bin/<name>@<version>/manifest.yaml`
    pub fn vfs_path_system(name: &str, version: &str) -> String {
        format!("/bin/{}@{}/manifest.yaml", name, version)
    }

    /// VFS path for a user-installed agent: `/users/<username>/bin/<name>@<version>/manifest.yaml`
    pub fn vfs_path_user(username: &str, name: &str, version: &str) -> String {
        format!("/users/{}/bin/{}@{}/manifest.yaml", username, name, version)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_YAML: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent
  author: avix-core
spec:
  entrypoint:
    type: llm-loop
  systemPromptPath: system-prompt.md
"#;

    const FULL_YAML: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: researcher
  version: 1.3.0
  description: General-purpose web & document researcher
  author: kernel-team
  createdAt: "2026-03-10T14:22:00Z"
  license: MIT
  tags: [research, web]
packaging:
  source: "github:avix/agents"
  signature: "sha256:abc123def456"
spec:
  systemPromptPath: system-prompt.md
  requestedCapabilities:
    - fs:*
    - web:*
  entrypoint:
    type: llm-loop
    modelRequirements:
      minContextWindow: 32000
      requiredCapabilities: [tool_use]
      recommended: claude-sonnet-4
    maxToolChain: 8
    maxTurnsPerGoal: 50
  tools:
    required: ["fs/read", "web/search", "web/fetch"]
    optional: ["code/interpreter"]
  memory:
    workingContext: dynamic
    episodicPersistence: true
    semanticStoreAccess: read-only
  snapshot:
    mode: per-turn
    restoreOnCrash: true
    compressionEnabled: true
  defaults:
    goalTemplate: "Research: {{topic}}"
    environment:
      temperature: 0.7
      topP: 0.9
      timeoutSec: 300
  visibility: public
  scope: system
"#;

    // T-MGA-01
    #[test]
    fn minimal_manifest_parses() {
        let m = AgentManifest::from_yaml(MINIMAL_YAML).unwrap();
        assert_eq!(m.kind, "Agent");
        assert_eq!(m.metadata.name, "echo-bot");
        assert_eq!(m.spec.entrypoint.entrypoint_type, EntrypointType::LlmLoop);
        assert_eq!(m.spec.entrypoint.max_tool_chain, 5);
        assert_eq!(m.spec.entrypoint.max_turns_per_goal, 20);
        assert_eq!(
            m.spec.system_prompt_path.as_deref(),
            Some("system-prompt.md")
        );
    }

    // T-MGA-02
    #[test]
    fn full_manifest_round_trips() {
        let m = AgentManifest::from_yaml(FULL_YAML).unwrap();
        let reparsed = AgentManifest::from_yaml(&m.to_yaml().unwrap()).unwrap();
        assert_eq!(reparsed.metadata.name, m.metadata.name);
        assert_eq!(reparsed.spec.entrypoint.max_tool_chain, 8);
        assert_eq!(
            reparsed.spec.tools.required,
            vec!["fs/read", "web/search", "web/fetch"]
        );
        assert_eq!(
            reparsed.spec.memory.semantic_store_access,
            SemanticStoreAccess::ReadOnly
        );
        assert_eq!(reparsed.spec.snapshot.mode, SnapshotMode::PerTurn);
        assert!(reparsed.spec.snapshot.restore_on_crash);
        assert_eq!(reparsed.spec.visibility.as_deref(), Some("public"));
        assert_eq!(reparsed.spec.scope.as_deref(), Some("system"));
        assert_eq!(reparsed.spec.requested_capabilities, vec!["fs:*", "web:*"]);
        assert_eq!(
            reparsed.packaging.source.as_deref(),
            Some("github:avix/agents")
        );
    }

    // T-MGA-03
    #[test]
    fn manifest_defaults_applied() {
        let yaml = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: minimal
  version: 0.1.0
spec: {}
"#;
        let m = AgentManifest::from_yaml(yaml).unwrap();
        assert_eq!(
            m.spec.entrypoint.model_requirements.min_context_window,
            8_000
        );
        assert_eq!(
            m.spec.entrypoint.model_requirements.recommended,
            "claude-sonnet-4"
        );
        assert!(m.spec.tools.required.is_empty());
        assert_eq!(m.spec.memory.working_context, WorkingContextMode::Dynamic);
        assert!(!m.spec.memory.episodic_persistence);
        assert_eq!(
            m.spec.memory.semantic_store_access,
            SemanticStoreAccess::None
        );
        assert_eq!(m.spec.snapshot.mode, SnapshotMode::Disabled);
        assert!(!m.spec.snapshot.restore_on_crash);
        assert!(m.spec.snapshot.compression_enabled);
        assert!((m.spec.defaults.environment.temperature - 0.7).abs() < f32::EPSILON);
        assert!(m.spec.system_prompt_path.is_none());
        assert!(m.spec.requested_capabilities.is_empty());
        assert!(m.packaging.source.is_none());
    }

    // T-MGA-04
    #[test]
    fn semantic_store_access_variants() {
        assert_eq!(
            serde_yaml::from_str::<SemanticStoreAccess>("read-only").unwrap(),
            SemanticStoreAccess::ReadOnly
        );
        assert_eq!(
            serde_yaml::from_str::<SemanticStoreAccess>("read-write").unwrap(),
            SemanticStoreAccess::ReadWrite
        );
        assert_eq!(
            serde_yaml::from_str::<SemanticStoreAccess>("none").unwrap(),
            SemanticStoreAccess::None
        );
    }

    // T-MGA-05
    #[test]
    fn snapshot_mode_variants() {
        assert_eq!(
            serde_yaml::from_str::<SnapshotMode>("per-turn").unwrap(),
            SnapshotMode::PerTurn
        );
        assert_eq!(
            serde_yaml::from_str::<SnapshotMode>("disabled").unwrap(),
            SnapshotMode::Disabled
        );
    }

    // T-MGA-06
    #[test]
    fn vfs_path_system_correct() {
        assert_eq!(
            AgentManifest::vfs_path_system("researcher", "1.3.0"),
            "/bin/researcher@1.3.0/manifest.yaml"
        );
    }

    // T-MGA-07
    #[test]
    fn vfs_path_user_correct() {
        assert_eq!(
            AgentManifest::vfs_path_user("alice", "coder", "2.1.0"),
            "/users/alice/bin/coder@2.1.0/manifest.yaml"
        );
    }

    // T-MGA-08
    #[test]
    fn unknown_kind_accepted_by_parser() {
        let yaml = r#"
apiVersion: avix/v1
kind: SomethingElse
metadata:
  name: x
  version: 1.0.0
spec: {}
"#;
        let m = AgentManifest::from_yaml(yaml).unwrap();
        assert_ne!(m.kind, "Agent");
    }

    // T-MGA-09
    #[test]
    fn tool_lists_are_independent() {
        let tools = ManifestTools {
            required: vec!["fs/read".into()],
            optional: vec!["web/search".into(), "code/interpreter".into()],
        };
        assert_eq!(tools.required.len(), 1);
        assert_eq!(tools.optional.len(), 2);
    }

    // T-MGA-10
    #[test]
    fn packaging_metadata_defaults_to_none() {
        let yaml = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: x
  version: 1.0.0
spec: {}
"#;
        let m = AgentManifest::from_yaml(yaml).unwrap();
        assert!(m.packaging.source.is_none());
        assert!(m.packaging.signature.is_none());
    }
}
