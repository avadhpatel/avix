# Manifest Gap A — AgentManifest Core Schema

> **Status:** Not started
> **Priority:** High — spawn-time resolution (Gap B) depends on these types
> **Depends on:** None
> **Affects:** `avix-core/src/agent_manifest/schema.rs` (new), `avix-core/src/agent_manifest/mod.rs` (new)

---

## Problem

There is no `AgentManifest` struct anywhere in the codebase. The static descriptor that defines
an agent's identity, model requirements, tool declarations, memory/snapshot behaviour, and default
prompts does not exist as a typed Rust structure.

The `params/defaults.rs` module contains `AgentDefaults`, `SnapshotDefaults`, etc., but these are
the **layered-defaults** system (system → crew → user → manifest layers) — they are not the
manifest itself. The manifest is a standalone YAML document authored by agent creators and
installed immutably at `/bin/<agent>/manifest.yaml`.

This gap defines the core schema types and YAML parsing only. Loading from VFS and spawn-time
resolution (signature check, tool grant intersection, model validation) is Gap B.

---

## Spec Deviations Captured Here

1. `spec.tools.required` / `spec.tools.optional` — the spec examples use underscored names
   (`web_search`, `file_read`). Per ADR-03, all Avix tool names use `/` as namespace separator
   (`web/search`, `fs/read`). Tests will assert the slash form. Agent manifests authored for
   v1 use the slash form.

2. `spec.memory.workingContext: fixed | dynamic` — the spec describes this as the agent's
   preferred context management mode. `fixed` means the system prompt + goal are set once at
   spawn and not grown. `dynamic` means the executor may expand context up to the model's max.
   This is a hint to `RuntimeExecutor`, not a hard enforcement in v1.

3. `spec.snapshot.mode: per-turn | disabled` — `per-turn` replaces the earlier boolean `enabled`
   in `SnapshotDefaults`. The layered params system (`SnapshotDefaults.enabled`) is a separate
   concern; it is updated in the snapshot gap plans.

---

## What Needs to Be Built

All types live in a new module: `crates/avix-core/src/agent_manifest/`.

### 1. `EntrypointType` enum

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntrypointType {
    LlmLoop,
}

impl Default for EntrypointType {
    fn default() -> Self { Self::LlmLoop }
}
```

### 2. `ModelRequirements`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequirements {
    #[serde(default = "default_min_context_window")]
    pub min_context_window: u32,          // default: 8000
    #[serde(default = "default_required_capabilities")]
    pub required_capabilities: Vec<String>, // ["tool_use"], ["tool_use", "vision"]
    #[serde(default = "default_recommended_model")]
    pub recommended: String,              // "claude-sonnet-4"
}

fn default_min_context_window() -> u32 { 8_000 }
fn default_required_capabilities() -> Vec<String> { vec!["tool_use".into()] }
fn default_recommended_model() -> String { "claude-sonnet-4".into() }
```

### 3. `ManifestEntrypoint`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntrypoint {
    #[serde(rename = "type", default)]
    pub entrypoint_type: EntrypointType,
    #[serde(default)]
    pub model_requirements: ModelRequirements,
    #[serde(default = "default_max_tool_chain")]
    pub max_tool_chain: u32,              // default: 5
    #[serde(default = "default_max_turns_per_goal")]
    pub max_turns_per_goal: u32,          // default: 20
}

fn default_max_tool_chain() -> u32 { 5 }
fn default_max_turns_per_goal() -> u32 { 20 }
```

> `type` is a reserved Rust keyword. Use `#[serde(rename = "type")]` on the `entrypoint_type` field.

### 4. `ManifestTools`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestTools {
    #[serde(default)]
    pub required: Vec<String>,   // Avix slash-form: ["fs/read", "web/search"]
    #[serde(default)]
    pub optional: Vec<String>,   // silently omitted if denied at spawn
}
```

### 5. `WorkingContextMode` and `SemanticStoreAccess` enums

```rust
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
```

### 6. `ManifestMemory`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMemory {
    #[serde(default)]
    pub working_context: WorkingContextMode,      // default: dynamic
    #[serde(default)]
    pub episodic_persistence: bool,               // default: false
    #[serde(default)]
    pub semantic_store_access: SemanticStoreAccess, // default: none
}
```

### 7. `SnapshotMode` enum and `ManifestSnapshot`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotMode {
    #[default]
    Disabled,
    PerTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestSnapshot {
    #[serde(default)]
    pub mode: SnapshotMode,                       // default: disabled
    #[serde(default)]
    pub restore_on_crash: bool,                   // default: false
    #[serde(default = "default_true")]
    pub compression_enabled: bool,                // default: true
}

fn default_true() -> bool { true }

impl Default for ManifestSnapshot {
    fn default() -> Self {
        Self {
            mode: SnapshotMode::Disabled,
            restore_on_crash: false,
            compression_enabled: true,
        }
    }
}
```

### 8. `ManifestEnvironment`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEnvironment {
    #[serde(default = "default_temperature")]
    pub temperature: f32,   // default: 0.7
    #[serde(default = "default_top_p")]
    pub top_p: f32,         // default: 0.9
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u32,   // default: 300
}

fn default_temperature() -> f32 { 0.7 }
fn default_top_p() -> f32 { 0.9 }
fn default_timeout_sec() -> u32 { 300 }

impl Default for ManifestEnvironment {
    fn default() -> Self {
        Self { temperature: 0.7, top_p: 0.9, timeout_sec: 300 }
    }
}
```

### 9. `ManifestDefaults`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,       // embedded from prompts/system.md at build time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_template: Option<String>,       // embedded from prompts/goal-template.md at build time
    #[serde(default)]
    pub environment: ManifestEnvironment,
}
```

### 10. `AgentManifestMetadata`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifestMetadata {
    pub name: String,
    pub version: String,                     // semver: "1.3.0"
    #[serde(default = "default_compat_version")]
    pub compatibility_version: u32,          // default: 1
    pub description: String,
    pub author: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,             // SPDX identifier
    pub signature: String,                   // "sha256:<hex>" — verified at load
}

fn default_compat_version() -> u32 { 1 }
```

### 11. `AgentManifestSpec`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifestSpec {
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
}
```

### 12. `AgentManifest` (top-level document)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,    // "avix/v1"
    pub kind: String,           // "AgentManifest"
    pub metadata: AgentManifestMetadata,
    pub spec: AgentManifestSpec,
}

impl AgentManifest {
    pub fn from_yaml(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// VFS path for a system-installed agent.
    /// `/bin/<agent>@<version>/manifest.yaml`
    pub fn vfs_path_system(name: &str, version: &str) -> String {
        format!("/bin/{}@{}/manifest.yaml", name, version)
    }

    /// VFS path for a user-installed agent.
    /// `/users/<username>/bin/<agent>@<version>/manifest.yaml`
    pub fn vfs_path_user(username: &str, name: &str, version: &str) -> String {
        format!("/users/{}/bin/{}@{}/manifest.yaml", username, name, version)
    }
}
```

---

## TDD Test Plan

File: `crates/avix-core/src/agent_manifest/schema.rs` under `#[cfg(test)]`

```rust
// T-MGA-01: Minimal manifest (no optional fields) parses successfully
#[test]
fn minimal_manifest_parses() {
    let yaml = r#"
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent
  author: avix-core
  createdAt: 2026-03-15T10:00:00Z
  signature: "sha256:abc123"
spec:
  entrypoint:
    type: llm-loop
  defaults:
    systemPrompt: "You are a helpful assistant."
"#;
    let m = AgentManifest::from_yaml(yaml).unwrap();
    assert_eq!(m.kind, "AgentManifest");
    assert_eq!(m.metadata.name, "echo-bot");
    assert_eq!(m.spec.entrypoint.entrypoint_type, EntrypointType::LlmLoop);
    assert_eq!(m.spec.entrypoint.max_tool_chain, 5);         // default
    assert_eq!(m.spec.entrypoint.max_turns_per_goal, 20);    // default
}

// T-MGA-02: Full manifest round-trips through YAML
#[test]
fn full_manifest_round_trips() {
    let yaml = r#"
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: researcher
  version: 1.3.0
  compatibilityVersion: 1
  description: General-purpose web & document researcher
  author: kernel-team
  createdAt: 2026-03-10T14:22:00Z
  license: MIT
  signature: "sha256:abc123def456"
spec:
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
    systemPrompt: "You are a research assistant."
    goalTemplate: "Research: {{topic}}"
    environment:
      temperature: 0.7
      topP: 0.9
      timeoutSec: 300
"#;
    let m = AgentManifest::from_yaml(yaml).unwrap();
    let reparsed = AgentManifest::from_yaml(&m.to_yaml().unwrap()).unwrap();
    assert_eq!(reparsed.metadata.name, m.metadata.name);
    assert_eq!(reparsed.spec.entrypoint.max_tool_chain, 8);
    assert_eq!(reparsed.spec.tools.required, vec!["fs/read", "web/search", "web/fetch"]);
    assert_eq!(reparsed.spec.memory.semantic_store_access, SemanticStoreAccess::ReadOnly);
    assert_eq!(reparsed.spec.snapshot.mode, SnapshotMode::PerTurn);
    assert!(reparsed.spec.snapshot.restore_on_crash);
}

// T-MGA-03: Defaults applied for missing fields
#[test]
fn manifest_defaults_applied() {
    let yaml = r#"
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: minimal
  version: 0.1.0
  description: Minimal
  author: test
  createdAt: 2026-01-01T00:00:00Z
  signature: "sha256:aaa"
spec: {}
"#;
    let m = AgentManifest::from_yaml(yaml).unwrap();
    assert_eq!(m.spec.entrypoint.model_requirements.min_context_window, 8_000);
    assert_eq!(m.spec.entrypoint.model_requirements.recommended, "claude-sonnet-4");
    assert_eq!(m.spec.tools.required, Vec::<String>::new());
    assert_eq!(m.spec.memory.working_context, WorkingContextMode::Dynamic);
    assert!(!m.spec.memory.episodic_persistence);
    assert_eq!(m.spec.memory.semantic_store_access, SemanticStoreAccess::None);
    assert_eq!(m.spec.snapshot.mode, SnapshotMode::Disabled);
    assert!(!m.spec.snapshot.restore_on_crash);
    assert!(m.spec.snapshot.compression_enabled);
    assert!((m.spec.defaults.environment.temperature - 0.7).abs() < f32::EPSILON);
}

// T-MGA-04: SemanticStoreAccess variants deserialise correctly
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

// T-MGA-05: SnapshotMode variants deserialise correctly
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

// T-MGA-06: vfs_path_system generates correct path
#[test]
fn vfs_path_system_correct() {
    assert_eq!(
        AgentManifest::vfs_path_system("researcher", "1.3.0"),
        "/bin/researcher@1.3.0/manifest.yaml"
    );
}

// T-MGA-07: vfs_path_user generates correct path
#[test]
fn vfs_path_user_correct() {
    assert_eq!(
        AgentManifest::vfs_path_user("alice", "coder", "2.1.0"),
        "/users/alice/bin/coder@2.1.0/manifest.yaml"
    );
}

// T-MGA-08: Unknown kind is rejected
#[test]
fn unknown_kind_rejected() {
    // serde should parse it but the caller validates kind == "AgentManifest"
    // (enforced in Gap B loader — schema itself accepts any string)
    let yaml = r#"
apiVersion: avix/v1
kind: SomethingElse
metadata:
  name: x
  version: 1.0.0
  description: x
  author: x
  createdAt: 2026-01-01T00:00:00Z
  signature: "sha256:aaa"
spec: {}
"#;
    let m = AgentManifest::from_yaml(yaml).unwrap();
    assert_ne!(m.kind, "AgentManifest");
}

// T-MGA-09: Optional tools list is independent from required list
#[test]
fn tool_lists_are_independent() {
    let tools = ManifestTools {
        required: vec!["fs/read".into()],
        optional: vec!["web/search".into(), "code/interpreter".into()],
    };
    assert_eq!(tools.required.len(), 1);
    assert_eq!(tools.optional.len(), 2);
}
```

---

## Implementation Notes

- Create `crates/avix-core/src/agent_manifest/mod.rs` and `schema.rs`. Re-export all public
  types from `mod.rs`.
- Add `pub mod agent_manifest;` to `crates/avix-core/src/lib.rs`.
- `type` is a reserved Rust keyword — use `#[serde(rename = "type")]` on `entrypoint_type`.
- `compatibility_version` in metadata serialises as `compatibilityVersion` via `camelCase`.
- `created_at` in metadata serialises as `createdAt` — use `#[serde(rename = "createdAt")]` or
  rely on the top-level `camelCase` rename (verify which `serde_yaml` handles correctly for
  `DateTime<Utc>` fields).
- Do NOT add any validation logic in this gap. Pure parsing/serialization only. Validation
  (signature check, tool name format enforcement, required fields at spawn) is Gap B.
- The `params/defaults.rs` `SnapshotDefaults.enabled` field is the *layered-defaults* system
  concept — leave it unchanged. `ManifestSnapshot.mode` is the manifest-native field.

---

## Success Criteria

- [ ] Minimal `AgentManifest` (no optional fields) parses without error (T-MGA-01)
- [ ] Full manifest round-trips through YAML with no data loss (T-MGA-02)
- [ ] All spec-default values are applied when fields are absent (T-MGA-03)
- [ ] `SemanticStoreAccess` kebab-case variants deserialise correctly (T-MGA-04)
- [ ] `SnapshotMode` kebab-case variants deserialise correctly (T-MGA-05)
- [ ] `vfs_path_system` returns correct VFS path (T-MGA-06)
- [ ] `vfs_path_user` returns correct VFS path (T-MGA-07)
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
