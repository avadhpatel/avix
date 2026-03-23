use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AvixError;

// ── Enums ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryRecordType {
    Episodic,
    Semantic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOutcome {
    Success,
    Partial,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryGrantScope {
    Session,
    Permanent,
}

// ── MemoryRecordIndex ─────────────────────────────────────────────────────────

/// Index metadata written exclusively by `memory.svc`. Never set by agents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordIndex {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fulltext_updated_at: Option<DateTime<Utc>>,
}

// ── MemoryRecordMetadata ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordMetadata {
    pub id: String,
    /// Field serialises as `"type"` in YAML (reserved Rust keyword).
    #[serde(rename = "type")]
    pub record_type: MemoryRecordType,
    pub agent_name: String,
    /// Informational only — not used for access control.
    pub agent_pid: u32,
    /// Username of the owning user.
    pub owner: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

// ── MemoryRecordSpec ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordSpec {
    /// Human-readable summary. Produced by the agent's LLM, never by memory.svc.
    pub content: String,

    // ── Episodic-only ─────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MemoryOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_goal: Option<String>,
    /// Tool names in Avix slash form: `["web/search", "fs/read"]` (ADR-03).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_used: Vec<String>,

    // ── Semantic-only ─────────────────────────────────────────────────────────
    /// Unique key within the agent's semantic store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<MemoryConfidence>,
    /// `None` means no expiry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_days: Option<u32>,

    // ── Index metadata — written by memory.svc only ───────────────────────────
    #[serde(default)]
    pub index: MemoryRecordIndex,
}

// ── MemoryRecord ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: MemoryRecordMetadata,
    pub spec: MemoryRecordSpec,
}

impl MemoryRecord {
    pub fn new(metadata: MemoryRecordMetadata, spec: MemoryRecordSpec) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "MemoryRecord".into(),
            metadata,
            spec,
        }
    }

    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// VFS path for episodic records.
    /// `/users/<owner>/memory/<agent-name>/episodic/<timestamp>-<id>.yaml`
    pub fn vfs_path_episodic(
        owner: &str,
        agent_name: &str,
        created_at: &DateTime<Utc>,
        id: &str,
    ) -> String {
        format!(
            "/users/{}/memory/{}/episodic/{}-{}.yaml",
            owner,
            agent_name,
            created_at.format("%Y-%m-%dT%H:%M:%SZ"),
            id
        )
    }

    /// VFS path for semantic records.
    /// `/users/<owner>/memory/<agent-name>/semantic/<key>.yaml`
    pub fn vfs_path_semantic(owner: &str, agent_name: &str, key: &str) -> String {
        format!("/users/{}/memory/{}/semantic/{}.yaml", owner, agent_name, key)
    }
}

/// Generate a memory record ID: `"mem-<8 char hex>"`.
pub fn new_memory_id() -> String {
    let id = uuid::Uuid::new_v4();
    format!("mem-{}", &id.simple().to_string()[..8])
}

// ── UserPreferenceModel ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferenceStructured {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_length: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cite_sources: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone_preference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expertise_areas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proactive_updates: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceCorrection {
    pub at: DateTime<Utc>,
    pub context: String,
    pub correction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferenceModelMetadata {
    pub agent_name: String,
    pub owner: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferenceModelSpec {
    /// Free-text prose summary injected verbatim into system prompt at spawn.
    pub summary: String,
    #[serde(default)]
    pub structured: UserPreferenceStructured,
    #[serde(default)]
    pub corrections: Vec<PreferenceCorrection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferenceModel {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: UserPreferenceModelMetadata,
    pub spec: UserPreferenceModelSpec,
}

impl UserPreferenceModel {
    pub fn new(metadata: UserPreferenceModelMetadata, spec: UserPreferenceModelSpec) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "UserPreferenceModel".into(),
            metadata,
            spec,
        }
    }

    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn vfs_path(owner: &str, agent_name: &str) -> String {
        format!(
            "/users/{}/memory/{}/preferences/user-model.yaml",
            owner, agent_name
        )
    }
}

// ── MemoryGrant ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantGrantor {
    pub agent_name: String,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantGrantee {
    pub agent_name: String,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantMetadata {
    pub id: String,
    pub granted_at: DateTime<Utc>,
    /// Username of the approving human.
    pub granted_by: String,
    pub hil_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantSpec {
    pub grantor: MemoryGrantGrantor,
    pub grantee: MemoryGrantGrantee,
    /// Record IDs (`mem-<id>`) that are accessible to the grantee.
    pub records: Vec<String>,
    pub scope: MemoryGrantScope,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrant {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: MemoryGrantMetadata,
    pub spec: MemoryGrantSpec,
}

impl MemoryGrant {
    pub fn new(metadata: MemoryGrantMetadata, spec: MemoryGrantSpec) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "MemoryGrant".into(),
            metadata,
            spec,
        }
    }

    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn vfs_path(owner: &str, agent_name: &str, grant_id: &str) -> String {
        format!(
            "/users/{}/memory/{}/grants/{}.yaml",
            owner, agent_name, grant_id
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_episodic_meta() -> MemoryRecordMetadata {
        MemoryRecordMetadata {
            id: "mem-abc123".into(),
            record_type: MemoryRecordType::Episodic,
            agent_name: "researcher".into(),
            agent_pid: 57,
            owner: "alice".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            session_id: "sess-xyz".into(),
            tags: vec!["research".into()],
            pinned: false,
        }
    }

    // T-MA-01: MemoryRecord (episodic) round-trips through YAML
    #[test]
    fn memory_record_episodic_round_trips() {
        let meta = make_episodic_meta();
        let spec = MemoryRecordSpec {
            content: "Completed web research.".into(),
            outcome: Some(MemoryOutcome::Success),
            related_goal: Some("Research topic".into()),
            tools_used: vec!["web/search".into()], // slash form, not underscore (ADR-03)
            key: None,
            confidence: None,
            ttl_days: None,
            index: MemoryRecordIndex::default(),
        };
        let record = MemoryRecord::new(meta, spec);
        let yaml = record.to_yaml().unwrap();
        let parsed = MemoryRecord::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.kind, "MemoryRecord");
        assert_eq!(parsed.metadata.record_type, MemoryRecordType::Episodic);
        assert!(parsed.spec.tools_used.contains(&"web/search".to_string()));
    }

    // T-MA-02: MemoryRecord (semantic) round-trips through YAML
    #[test]
    fn memory_record_semantic_round_trips() {
        let meta = MemoryRecordMetadata {
            id: "mem-def456".into(),
            record_type: MemoryRecordType::Semantic,
            agent_name: "researcher".into(),
            agent_pid: 57,
            owner: "alice".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            session_id: "sess-xyz".into(),
            tags: vec![],
            pinned: false,
        };
        let spec = MemoryRecordSpec {
            content: "Project Alpha deadline is April 30, 2026.".into(),
            key: Some("project-alpha-deadline".into()),
            confidence: Some(MemoryConfidence::High),
            ttl_days: None,
            outcome: None,
            related_goal: None,
            tools_used: vec![],
            index: MemoryRecordIndex::default(),
        };
        let record = MemoryRecord::new(meta, spec);
        let yaml = record.to_yaml().unwrap();
        let parsed = MemoryRecord::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.metadata.record_type, MemoryRecordType::Semantic);
        assert_eq!(
            parsed.spec.key.as_deref(),
            Some("project-alpha-deadline")
        );
        assert_eq!(parsed.spec.confidence, Some(MemoryConfidence::High));
    }

    // T-MA-03: UserPreferenceModel round-trips through YAML
    #[test]
    fn user_preference_model_round_trips() {
        let meta = UserPreferenceModelMetadata {
            agent_name: "researcher".into(),
            owner: "alice".into(),
            updated_at: Utc::now(),
        };
        let spec = UserPreferenceModelSpec {
            summary: "Alice prefers concise markdown output.".into(),
            structured: UserPreferenceStructured {
                output_format: Some("markdown".into()),
                preferred_length: Some("concise".into()),
                ..Default::default()
            },
            corrections: vec![],
        };
        let model = UserPreferenceModel::new(meta, spec);
        let yaml = model.to_yaml().unwrap();
        let parsed = UserPreferenceModel::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.kind, "UserPreferenceModel");
        assert_eq!(
            parsed.spec.structured.output_format.as_deref(),
            Some("markdown")
        );
    }

    // T-MA-04: MemoryGrant round-trips through YAML
    #[test]
    fn memory_grant_round_trips() {
        let meta = MemoryGrantMetadata {
            id: "grant-001".into(),
            granted_at: Utc::now(),
            granted_by: "alice".into(),
            hil_id: "hil-xyz".into(),
        };
        let spec = MemoryGrantSpec {
            grantor: MemoryGrantGrantor {
                agent_name: "researcher".into(),
                owner: "alice".into(),
            },
            grantee: MemoryGrantGrantee {
                agent_name: "writer".into(),
                owner: "alice".into(),
            },
            records: vec!["mem-abc123".into()],
            scope: MemoryGrantScope::Session,
            session_id: "sess-xyz".into(),
            expires_at: None,
        };
        let grant = MemoryGrant::new(meta, spec);
        let yaml = grant.to_yaml().unwrap();
        let parsed = MemoryGrant::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.kind, "MemoryGrant");
        assert_eq!(parsed.spec.scope, MemoryGrantScope::Session);
        assert_eq!(parsed.spec.records, vec!["mem-abc123"]);
    }

    // T-MA-05: vfs_path_episodic generates correct path
    #[test]
    fn episodic_vfs_path_correct() {
        let dt = Utc.with_ymd_and_hms(2026, 3, 22, 14, 30, 0).unwrap();
        let path = MemoryRecord::vfs_path_episodic("alice", "researcher", &dt, "abc123");
        assert_eq!(
            path,
            "/users/alice/memory/researcher/episodic/2026-03-22T14:30:00Z-abc123.yaml"
        );
    }

    // T-MA-06: vfs_path_semantic generates correct path
    #[test]
    fn semantic_vfs_path_correct() {
        let path =
            MemoryRecord::vfs_path_semantic("alice", "researcher", "project-alpha-deadline");
        assert_eq!(
            path,
            "/users/alice/memory/researcher/semantic/project-alpha-deadline.yaml"
        );
    }

    // T-MA-07: UserPreferenceModel.vfs_path correct
    #[test]
    fn preference_model_vfs_path_correct() {
        let path = UserPreferenceModel::vfs_path("alice", "researcher");
        assert_eq!(
            path,
            "/users/alice/memory/researcher/preferences/user-model.yaml"
        );
    }

    // T-MA-09: MemoryRecordType serialises to lowercase
    #[test]
    fn record_type_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&MemoryRecordType::Episodic)
                .unwrap()
                .trim(),
            "episodic"
        );
        assert_eq!(
            serde_yaml::to_string(&MemoryRecordType::Semantic)
                .unwrap()
                .trim(),
            "semantic"
        );
    }

    // new_memory_id produces "mem-" prefix and 8 hex chars
    #[test]
    fn memory_id_format() {
        let id = new_memory_id();
        assert!(id.starts_with("mem-"));
        let hex = &id["mem-".len()..];
        assert_eq!(hex.len(), 8);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
