# Memory Gap A — Core Schema Types

> **Status:** Complete
> **Priority:** High — all other memory gaps depend on these types
> **Depends on:** None
> **Affects:** `avix-core/src/memory_svc/schema.rs` (new), `avix-core/src/config/kernel.rs`

---

## Problem

No `MemoryRecord`, `UserPreferenceModel`, or `MemoryGrant` structs exist. The existing
`MemoryConfig` in `kernel.rs` has different fields than the spec. `MemoryDefaults` and
`MemoryLimits` in the params module are also misaligned. All downstream gaps (service,
VFS, spawn injection, retrieval) require these types first.

---

## Spec Correction Captured Here

The spec's `MemoryRecord.spec.toolsUsed` example shows `[web_search, web_fetch]` with
underscores. Per ADR-03 all Avix tool names use `/` as the namespace separator. The
correct value is `["web/search", "web/fetch"]`. The Rust type will use `Vec<String>` and
the tests will assert the slash form.

---

## What Needs to Be Built

### 1. `MemoryRecordType` enum

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryRecordType {
    Episodic,
    Semantic,
}
```

### 2. `MemoryOutcome` enum (episodic only)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryOutcome {
    Success,
    Partial,
    Failure,
}
```

### 3. `MemoryConfidence` enum (semantic only)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryConfidence {
    High,
    Medium,
    Low,
}
```

### 4. `MemoryRecordIndex`

Written exclusively by `memory.svc`. Never set by agents.

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordIndex {
    pub vector_model: Option<String>,
    pub vector_updated_at: Option<DateTime<Utc>>,
    pub fulltext_updated_at: Option<DateTime<Utc>>,
}
```

### 5. `MemoryRecordMetadata`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordMetadata {
    pub id: String,                              // "mem-<nanoid>"
    pub record_type: MemoryRecordType,           // field name: "type" via rename
    pub agent_name: String,
    pub agent_pid: u32,                          // informational only
    pub owner: String,                           // username
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub session_id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}
```

> `type` is a reserved Rust keyword. Use `#[serde(rename = "type")]` on `record_type`.

### 6. `MemoryRecordSpec`

Combines episodic and semantic fields. Fields only relevant to one type are `Option`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecordSpec {
    /// Human-readable summary — always present. Produced by agent LLM, never by memory.svc.
    pub content: String,

    // ── Episodic-only ─────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MemoryOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_goal: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_used: Vec<String>,        // Avix names: ["web/search", "fs/read"]

    // ── Semantic-only ─────────────────────────────────────────────────────────
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,            // unique within agent's semantic store
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<MemoryConfidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_days: Option<u32>,          // None = no expiry

    // ── Index metadata — written by memory.svc only ───────────────────────────
    #[serde(default)]
    pub index: MemoryRecordIndex,
}
```

### 7. `MemoryRecord`

The on-disk YAML document.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecord {
    #[serde(rename = "apiVersion")]
    pub api_version: String,            // "avix/v1"
    pub kind: String,                   // "MemoryRecord"
    pub metadata: MemoryRecordMetadata,
    pub spec: MemoryRecordSpec,
}

impl MemoryRecord {
    pub fn new(metadata: MemoryRecordMetadata, spec: MemoryRecordSpec) -> Self {
        Self { api_version: "avix/v1".into(), kind: "MemoryRecord".into(), metadata, spec }
    }

    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// VFS path for episodic records.
    /// `/users/<owner>/memory/<agent-name>/episodic/<timestamp>-<id>.yaml`
    pub fn vfs_path_episodic(owner: &str, agent_name: &str, created_at: &DateTime<Utc>, id: &str) -> String {
        format!(
            "/users/{}/memory/{}/episodic/{}-{}.yaml",
            owner, agent_name, created_at.format("%Y-%m-%dT%H:%M:%SZ"), id
        )
    }

    /// VFS path for semantic records.
    /// `/users/<owner>/memory/<agent-name>/semantic/<key>.yaml`
    pub fn vfs_path_semantic(owner: &str, agent_name: &str, key: &str) -> String {
        format!("/users/{}/memory/{}/semantic/{}.yaml", owner, agent_name, key)
    }
}

/// Generate a memory record ID: "mem-<8 char hex>".
pub fn new_memory_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    format!("mem-{:08x}", nanos)
}
```

### 8. `UserPreferenceStructured`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreferenceStructured {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,       // "markdown" | "plain" | "json"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_length: Option<String>,    // "concise" | "detailed"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cite_sources: Option<String>,        // "always" | "when-relevant" | "never"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone_preference: Option<String>,     // "professional" | "casual"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_language: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expertise_areas: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proactive_updates: Option<bool>,
}
```

### 9. `PreferenceCorrection`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferenceCorrection {
    pub at: DateTime<Utc>,
    pub context: String,
    pub correction: String,
}
```

### 10. `UserPreferenceModel`

```rust
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
        Self { api_version: "avix/v1".into(), kind: "UserPreferenceModel".into(), metadata, spec }
    }

    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn vfs_path(owner: &str, agent_name: &str) -> String {
        format!("/users/{}/memory/{}/preferences/user-model.yaml", owner, agent_name)
    }
}
```

### 11. `MemoryGrantScope`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryGrantScope {
    Session,
    Permanent,
}
```

### 12. `MemoryGrant`

```rust
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
    pub granted_by: String,    // username of approving human
    pub hil_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGrantSpec {
    pub grantor: MemoryGrantGrantor,
    pub grantee: MemoryGrantGrantee,
    pub records: Vec<String>,           // Vec<mem-id>
    pub scope: MemoryGrantScope,
    pub session_id: String,
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
    pub fn vfs_path(owner: &str, agent_name: &str, grant_id: &str) -> String {
        format!("/users/{}/memory/{}/grants/{}.yaml", owner, agent_name, grant_id)
    }
}
```

### 13. Update `KernelConfig.memory` in `config/kernel.rs`

Replace the existing lean `MemoryConfig` with the spec-aligned version:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEpisodicConfig {
    #[serde(default = "default_max_retention_days")]
    pub max_retention_days: u32,        // default: 30
    #[serde(default = "default_max_records_per_agent")]
    pub max_records_per_agent: u32,     // default: 10000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySemanticConfig {
    #[serde(default = "default_max_facts_per_agent")]
    pub max_facts_per_agent: u32,       // default: 5000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRetrievalConfig {
    #[serde(default = "default_retrieval_limit")]
    pub default_limit: u32,             // default: 5
    #[serde(default = "default_max_retrieval_limit")]
    pub max_limit: u32,                 // default: 20
    #[serde(default = "default_candidate_fetch_k")]
    pub candidate_fetch_k: u32,         // default: 20
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,                     // default: 60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySpawnConfig {
    #[serde(default = "default_episodic_context_records")]
    pub episodic_context_records: u32,  // default: 5
    #[serde(default = "default_true")]
    pub preferences_enabled: bool,
    #[serde(default = "default_true")]
    pub pinned_facts_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySharingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hil_timeout_sec")]
    pub hil_timeout_sec: u64,           // default: 600
    #[serde(default)]
    pub cross_user_enabled: bool,       // always false in v1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    #[serde(default = "default_context_limit")]
    pub default_context_limit: u32,     // default: 200000
    #[serde(default)]
    pub episodic: MemoryEpisodicConfig,
    #[serde(default)]
    pub semantic: MemorySemanticConfig,
    #[serde(default)]
    pub retrieval: MemoryRetrievalConfig,
    #[serde(default)]
    pub spawn: MemorySpawnConfig,
    #[serde(default)]
    pub sharing: MemorySharingConfig,
}
```

> The existing `MemoryConfig` fields (`eviction_policy`, `shared_memory_path`) are
> removed — they have no corresponding concept in the spec. Existing tests referencing
> them must be updated.

### 14. Update `AgentManifest` memory block in `params/`

The current `MemoryDefaults` and `MemoryLimits` do not match the spec's `AgentManifest.spec.memory` block. Replace with:

```rust
// In params/defaults.rs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEpisodicDefaults {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_log_on_session_end: bool,
    pub retention_days: Option<u32>,    // None = use kernel default
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemorySemanticDefaults {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_read_write")]
    pub access: String,                 // "none" | "read-only" | "read-write"
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPreferencesDefaults {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub auto_inject_at_spawn: bool,
    #[serde(default = "default_true")]
    pub auto_capture_corrections: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCrewDefaults {
    #[serde(default = "default_true")]
    pub read_shared: bool,
    #[serde(default)]
    pub write_shared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemorySharingDefaults {
    #[serde(default)]
    pub can_request: bool,
    #[serde(default)]
    pub can_receive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDefaults {
    #[serde(default)]
    pub episodic: MemoryEpisodicDefaults,
    #[serde(default)]
    pub semantic: MemorySemanticDefaults,
    #[serde(default)]
    pub preferences: MemoryPreferencesDefaults,
    #[serde(default)]
    pub crew: MemoryCrewDefaults,
    #[serde(default)]
    pub sharing: MemorySharingDefaults,
}
```

### 15. `ResolvedMemory` in `params/resolver.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMemory {
    pub episodic_enabled: bool,
    pub auto_log_on_session_end: bool,
    pub retention_days: u32,            // resolved from manifest or kernel default
    pub semantic_enabled: bool,
    pub semantic_access: String,        // "none" | "read-only" | "read-write"
    pub preferences_enabled: bool,
    pub auto_inject_at_spawn: bool,
    pub auto_capture_corrections: bool,
    pub crew_read_shared: bool,
    pub crew_write_shared: bool,
    pub sharing_can_request: bool,
    pub sharing_can_receive: bool,
}
```

---

## Note: Conversation History vs Memory

This gap does not change how conversation history works. `RuntimeExecutor` already
holds `conversation_history: Vec<Message>` in-process and passes it on every
`llm/complete` call (stateless LLM requires full context each call). This in-session
history is ephemeral and owned by the executor.

The memory service is a separate, complementary layer:
- **In-session:** `RuntimeExecutor.conversation_history` (in-memory, not persisted)
- **Cross-session:** `memory.svc` episodic/semantic/preference records (VFS-persisted)

At session end, `RuntimeExecutor` asks the LLM to summarize `conversation_history`
and writes that summary — not the raw transcript — via `memory/log-event`.

---

## TDD Test Plan

File: `crates/avix-core/src/memory_svc/schema.rs` under `#[cfg(test)]`

```rust
// T-MA-01: MemoryRecord (episodic) round-trips through YAML
#[test]
fn memory_record_episodic_round_trips() {
    let meta = MemoryRecordMetadata {
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
    };
    let spec = MemoryRecordSpec {
        content: "Completed web research.".into(),
        outcome: Some(MemoryOutcome::Success),
        related_goal: Some("Research topic".into()),
        tools_used: vec!["web/search".into()],  // slash form, not underscore
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
    let spec = MemoryRecordSpec {
        content: "Project Alpha deadline is April 30, 2026.".into(),
        key: Some("project-alpha-deadline".into()),
        confidence: Some(MemoryConfidence::High),
        ttl_days: None,
        outcome: None, related_goal: None,
        tools_used: vec![],
        index: MemoryRecordIndex::default(),
    };
    // ... build record, round-trip, assert key and confidence
}

// T-MA-03: UserPreferenceModel round-trips through YAML
#[test]
fn user_preference_model_round_trips() { ... }

// T-MA-04: MemoryGrant round-trips through YAML
#[test]
fn memory_grant_round_trips() { ... }

// T-MA-05: vfs_path_episodic generates correct path
#[test]
fn episodic_vfs_path_correct() {
    let dt = Utc.with_ymd_and_hms(2026, 3, 22, 14, 30, 0).unwrap();
    let path = MemoryRecord::vfs_path_episodic("alice", "researcher", &dt, "abc123");
    assert_eq!(path, "/users/alice/memory/researcher/episodic/2026-03-22T14:30:00Z-abc123.yaml");
}

// T-MA-06: vfs_path_semantic generates correct path
#[test]
fn semantic_vfs_path_correct() {
    let path = MemoryRecord::vfs_path_semantic("alice", "researcher", "project-alpha-deadline");
    assert_eq!(path, "/users/alice/memory/researcher/semantic/project-alpha-deadline.yaml");
}

// T-MA-07: UserPreferenceModel.vfs_path correct
#[test]
fn preference_model_vfs_path_correct() {
    let path = UserPreferenceModel::vfs_path("alice", "researcher");
    assert_eq!(path, "/users/alice/memory/researcher/preferences/user-model.yaml");
}

// T-MA-08: MemoryConfig defaults are correct
#[test]
fn memory_config_defaults() {
    let cfg: MemoryConfig = serde_yaml::from_str("{}").unwrap();
    assert_eq!(cfg.default_context_limit, 200_000);
    assert_eq!(cfg.episodic.max_retention_days, 30);
    assert_eq!(cfg.retrieval.default_limit, 5);
    assert_eq!(cfg.retrieval.max_limit, 20);
    assert_eq!(cfg.retrieval.rrf_k, 60);
    assert!(!cfg.sharing.cross_user_enabled);
}

// T-MA-09: MemoryRecordType serialises to lowercase
#[test]
fn record_type_lowercase() {
    assert_eq!(serde_yaml::to_string(&MemoryRecordType::Episodic).unwrap().trim(), "episodic");
    assert_eq!(serde_yaml::to_string(&MemoryRecordType::Semantic).unwrap().trim(), "semantic");
}
```

---

## Implementation Notes

- Create `crates/avix-core/src/memory_svc/mod.rs` and `schema.rs`. All memory types live
  in `memory_svc::schema`.
- The `"type"` field in `MemoryRecordMetadata` conflicts with Rust's keyword — use
  `#[serde(rename = "type")]` on the `record_type` field.
- The `new_memory_id()` function should use a proper random source. Use
  `uuid::Uuid::new_v4()` (already in the dependency graph) and take the first 8 hex
  chars: `format!("mem-{}", &uuid.simple().to_string()[..8])`.
- `MemoryConfig` fields in `kernel.rs` lose `eviction_policy` and `shared_memory_path`.
  Update existing tests that reference these fields. The `EvictionPolicy` enum may be
  deleted if it has no other users.

---

## Success Criteria

- [ ] `MemoryRecord` (episodic + semantic) round-trips through YAML (T-MA-01, T-MA-02)
- [ ] `UserPreferenceModel` round-trips through YAML (T-MA-03)
- [ ] `MemoryGrant` round-trips through YAML (T-MA-04)
- [ ] `vfs_path_episodic` generates correct path (T-MA-05)
- [ ] `vfs_path_semantic` generates correct path (T-MA-06)
- [ ] `UserPreferenceModel.vfs_path` correct (T-MA-07)
- [ ] `MemoryConfig` defaults match spec (T-MA-08)
- [ ] `MemoryRecordType` serialises lowercase (T-MA-09)
- [ ] `cargo clippy --workspace -- -D warnings` passes
