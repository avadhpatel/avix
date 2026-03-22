# Param Gap B — Typed Defaults and Limits Structs

> **Status:** Not started
> **Priority:** High — required before resolution engine (Gap C)
> **Affects:** new `avix-core/src/params/defaults.rs`, new `avix-core/src/params/limits.rs`, `avix-core/src/bootstrap/phase1.rs`

---

## Problem

`bootstrap/phase1.rs` writes defaults and limits to `/kernel/defaults/` and
`/kernel/limits/` as raw YAML strings with hard-coded literal values. There are no typed
Rust structs for `Defaults` or `Limits` — values cannot be validated, merged, or read
back without re-parsing raw YAML.

The spec (`docs/spec/param-defaults.md`, `docs/spec/param-limits.md`) defines:

- `Defaults` — a layered config file at system (`/kernel/defaults/<kind>.yaml`), user
  (`/users/<u>/defaults.yaml`), crew (`/crews/<crew>/defaults.yaml`), and service
  (`/services/<svc>/defaults.yaml`) layers.
- `Limits` — a kernel-owned constraint file at system (`/kernel/limits/<kind>.yaml`),
  user, crew, and service layers. Limits can only **narrow** — never widen.

Neither struct exists in the Rust codebase. The resolution engine (Gap C) cannot be
built until these types exist.

---

## What Needs to Be Built

### 1. New `params` module (`avix-core/src/params/mod.rs`)

```rust
pub mod defaults;
pub mod limits;
pub mod constraint;

pub use defaults::{AgentDefaults, ToolDefaults, DefaultsFile};
pub use limits::{AgentLimits, ToolLimits, LimitsFile};
pub use constraint::{Constraint, RangeConstraint, EnumConstraint, SetConstraint, BoolConstraint};
```

### 2. Constraint types (`params/constraint.rs`)

Mirror the spec's constraint type system:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    Range(RangeConstraint),
    Enum(EnumConstraint),
    Set(SetConstraint),
    Bool(BoolConstraint),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RangeConstraint {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl RangeConstraint {
    /// Returns the tightest (intersection) range of self and other.
    pub fn intersect(&self, other: &Self) -> Self { ... }
    /// Returns true if `value` satisfies this constraint.
    pub fn allows(&self, value: f64) -> bool { ... }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumConstraint {
    pub values: Vec<String>,
}

impl EnumConstraint {
    pub fn intersect(&self, other: &Self) -> Self { ... }  // intersection of allowed values
    pub fn allows(&self, value: &str) -> bool { ... }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetConstraint {
    pub allowed: Vec<String>,
}

impl SetConstraint {
    pub fn intersect(&self, other: &Self) -> Self { ... }
    pub fn allows(&self, item: &str) -> bool { ... }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoolConstraint {
    pub value: Option<bool>,   // None = not locked; Some(v) = locked to v
}
```

### 3. AgentDefaults struct (`params/defaults.rs`)

```rust
/// Mirrors the `defaults:` block in a Defaults file targeting `agent-manifest`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentDefaults {
    pub entrypoint: EntrypointDefaults,
    pub memory: MemoryDefaults,
    pub snapshot: SnapshotDefaults,
    pub environment: EnvironmentDefaults,
    pub permissions_hint: PermissionsHintDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EntrypointDefaults {
    pub model_preference: Option<String>,
    pub min_context_tokens: Option<u32>,
    pub max_tool_chain: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryDefaults {
    pub working_context: Option<String>,
    pub episodic_persistence: Option<bool>,
    pub semantic_store_access: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotDefaults {
    pub enabled: Option<bool>,
    pub auto_snapshot_interval_sec: Option<u32>,
    pub restore_on_crash: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvironmentDefaults {
    pub temperature: Option<f32>,
    pub timeout_sec: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PermissionsHintDefaults {
    pub owner: Option<String>,
    pub crew: Option<String>,
    pub world: Option<String>,
}

/// Full Defaults file envelope (wraps AgentDefaults or ToolDefaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsFile {
    pub api_version: String,
    pub kind: String,   // "Defaults"
    pub metadata: DefaultsMetadata,
    pub defaults: serde_yaml::Value,  // target-specific; parse into AgentDefaults or ToolDefaults
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsMetadata {
    pub target: String,      // "agent-manifest" | "tool"
    pub layer: DefaultsLayer,
    pub owner: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultsLayer {
    System,
    User,
    Crew,
    Service,
}

impl DefaultsFile {
    pub fn from_str(s: &str) -> Result<Self, AvixError> { ... }

    /// Parse the `defaults:` block into `AgentDefaults`.
    pub fn as_agent_defaults(&self) -> Result<AgentDefaults, AvixError> { ... }
}
```

### 4. AgentLimits struct (`params/limits.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentLimits {
    pub entrypoint: EntrypointLimits,
    pub tools: ToolsLimits,
    pub memory: MemoryLimits,
    pub snapshot: SnapshotLimits,
    pub environment: EnvironmentLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EntrypointLimits {
    pub model_preference: Option<EnumConstraint>,
    pub min_context_tokens: Option<RangeConstraint>,
    pub max_tool_chain: Option<RangeConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsLimits {
    pub required: Option<SetConstraint>,
    pub optional: Option<SetConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryLimits {
    pub working_context: Option<EnumConstraint>,
    pub semantic_store_access: Option<EnumConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotLimits {
    pub enabled: Option<BoolConstraint>,
    pub auto_snapshot_interval_sec: Option<RangeConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvironmentLimits {
    pub temperature: Option<RangeConstraint>,
    pub timeout_sec: Option<RangeConstraint>,
}

impl AgentLimits {
    /// Returns the tightest combination of self and other (for multi-crew intersection).
    pub fn intersect(&self, other: &AgentLimits) -> AgentLimits { ... }

    /// Returns Err if any field in `defaults` violates a constraint in `self`.
    pub fn check_defaults(&self, defaults: &AgentDefaults) -> Result<(), Vec<LimitViolation>> { ... }
}

pub struct LimitViolation {
    pub field: String,         // e.g. "entrypoint.maxToolChain"
    pub value: String,
    pub constraint: Constraint,
}

/// Full Limits file envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsFile {
    pub api_version: String,
    pub kind: String,   // "Limits"
    pub metadata: LimitsMetadata,
    pub limits: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsMetadata {
    pub target: String,
    pub layer: LimitsLayer,
    pub owner: Option<String>,
    pub updated_at: String,
    pub updated_by: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LimitsLayer { System, User, Crew, Service }

impl LimitsFile {
    pub fn from_str(s: &str) -> Result<Self, AvixError> { ... }
    pub fn as_agent_limits(&self) -> Result<AgentLimits, AvixError> { ... }
}
```

### 5. Update bootstrap Phase 1 to use typed structs

`bootstrap/phase1.rs` currently writes two hard-coded YAML strings. Replace with:

```rust
// Compiled-in system defaults
const SYSTEM_AGENT_DEFAULTS: AgentDefaults = AgentDefaults {
    entrypoint: EntrypointDefaults {
        model_preference: Some("claude-sonnet-4".into()),
        min_context_tokens: Some(8000),
        max_tool_chain: Some(5),
    },
    environment: EnvironmentDefaults {
        temperature: Some(0.7),
        timeout_sec: Some(300),
    },
    ..Default::default()
};

// Compiled-in system limits
const SYSTEM_AGENT_LIMITS: AgentLimits = AgentLimits {
    entrypoint: EntrypointLimits {
        min_context_tokens: Some(RangeConstraint { min: Some(1000.0), max: Some(200000.0) }),
        max_tool_chain: Some(RangeConstraint { min: Some(1.0), max: Some(200.0) }),
        ..Default::default()
    },
    environment: EnvironmentLimits {
        temperature: Some(RangeConstraint { min: Some(0.0), max: Some(2.0) }),
        timeout_sec: Some(RangeConstraint { min: Some(30.0), max: Some(600.0) }),
        ..Default::default()
    },
    ..Default::default()
};
```

Serialize these structs to YAML and write them to the VFS in phase 1 instead of using
raw string templates.

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/params.rs` (new file).

```rust
// T-B-01: DefaultsFile parses spec example YAML
#[test]
fn defaults_file_parses_spec_example() {
    let yaml = include_str!("fixtures/defaults_system.yaml");
    let file = DefaultsFile::from_str(yaml).unwrap();
    assert_eq!(file.metadata.target, "agent-manifest");
    let agent_defaults = file.as_agent_defaults().unwrap();
    assert_eq!(agent_defaults.entrypoint.max_tool_chain, Some(5));
    assert_eq!(agent_defaults.environment.temperature, Some(0.7));
}

// T-B-02: User-layer DefaultsFile parses correctly
#[test]
fn defaults_file_user_layer_parses() {
    let yaml = include_str!("fixtures/defaults_user_alice.yaml");
    let file = DefaultsFile::from_str(yaml).unwrap();
    assert_eq!(file.metadata.layer, DefaultsLayer::User);
    assert_eq!(file.metadata.owner, Some("alice".into()));
    let d = file.as_agent_defaults().unwrap();
    assert_eq!(d.entrypoint.max_tool_chain, Some(8));
}

// T-B-03: LimitsFile parses spec example YAML
#[test]
fn limits_file_parses_spec_example() {
    let yaml = include_str!("fixtures/limits_system.yaml");
    let file = LimitsFile::from_str(yaml).unwrap();
    let limits = file.as_agent_limits().unwrap();
    let max = limits.entrypoint.max_tool_chain.unwrap();
    assert_eq!(max.max, Some(10.0));
}

// T-B-04: RangeConstraint.intersect produces tightest range
#[test]
fn range_constraint_intersect_tightest() {
    let a = RangeConstraint { min: Some(1.0), max: Some(10.0) };
    let b = RangeConstraint { min: Some(3.0), max: Some(7.0) };
    let c = a.intersect(&b);
    assert_eq!(c.min, Some(3.0));
    assert_eq!(c.max, Some(7.0));
}

// T-B-05: EnumConstraint.intersect produces intersection
#[test]
fn enum_constraint_intersect_intersection() {
    let a = EnumConstraint { values: vec!["sonnet".into(), "haiku".into()] };
    let b = EnumConstraint { values: vec!["haiku".into(), "opus".into()] };
    let c = a.intersect(&b);
    assert_eq!(c.values, vec!["haiku"]);
}

// T-B-06: AgentLimits.intersect applies tightest across crews
#[test]
fn agent_limits_multi_crew_tightest() {
    let researchers = AgentLimits {
        entrypoint: EntrypointLimits {
            max_tool_chain: Some(RangeConstraint { min: Some(1.0), max: Some(10.0) }),
            ..Default::default()
        },
        ..Default::default()
    };
    let writers = AgentLimits {
        entrypoint: EntrypointLimits {
            max_tool_chain: Some(RangeConstraint { min: Some(1.0), max: Some(5.0) }),
            ..Default::default()
        },
        ..Default::default()
    };
    let effective = researchers.intersect(&writers);
    assert_eq!(effective.entrypoint.max_tool_chain.unwrap().max, Some(5.0));
}

// T-B-07: AgentLimits.check_defaults returns violation when defaults exceed limits
#[test]
fn agent_limits_check_defaults_violation() {
    let limits = AgentLimits {
        entrypoint: EntrypointLimits {
            max_tool_chain: Some(RangeConstraint { min: None, max: Some(5.0) }),
            ..Default::default()
        },
        ..Default::default()
    };
    let defaults = AgentDefaults {
        entrypoint: EntrypointDefaults {
            max_tool_chain: Some(20),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = limits.check_defaults(&defaults);
    assert!(result.is_err());
    let violations = result.unwrap_err();
    assert_eq!(violations[0].field, "entrypoint.maxToolChain");
}

// T-B-08: Bootstrap phase 1 writes typed YAML that round-trips
#[tokio::test]
async fn phase1_defaults_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = MemFs::new(dir.path());
    run_phase1(&vfs).await.unwrap();
    let raw = vfs.read("/kernel/defaults/agent-manifest.yaml").await.unwrap();
    let file = DefaultsFile::from_str(&String::from_utf8(raw).unwrap()).unwrap();
    let d = file.as_agent_defaults().unwrap();
    assert_eq!(d.entrypoint.max_tool_chain, Some(5));
}

// T-B-09: Bootstrap phase 1 writes typed limits YAML that round-trips
#[tokio::test]
async fn phase1_limits_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = MemFs::new(dir.path());
    run_phase1(&vfs).await.unwrap();
    let raw = vfs.read("/kernel/limits/agent-manifest.yaml").await.unwrap();
    let file = LimitsFile::from_str(&String::from_utf8(raw).unwrap()).unwrap();
    let limits = file.as_agent_limits().unwrap();
    assert!(limits.entrypoint.max_tool_chain.is_some());
}
```

---

## Implementation Notes

- All `Option<T>` fields in `AgentDefaults` and `AgentLimits` model the "not set at
  this layer" case — `None` means "no value at this layer, defer to next".
- Do NOT implement `Default` with hard-coded values for `AgentDefaults` — use
  `#[derive(Default)]` so all fields are `None`. The system-level defaults are
  constants defined in phase1, not struct defaults.
- `AgentLimits::intersect` must handle the case where one side has `None` for a field
  (meaning no constraint at that layer) — treat `None` as "unconstrained" and return the
  other side's constraint.
- Serialize field names to camelCase to match the spec YAML format.

---

## Success Criteria

- [ ] `DefaultsFile::from_str` parses spec examples without errors
- [ ] `LimitsFile::from_str` parses spec examples without errors
- [ ] `AgentLimits::intersect` correctly applies tightest-constraint logic (T-B-04 through T-B-06)
- [ ] `AgentLimits::check_defaults` detects violations (T-B-07)
- [ ] Phase 1 writes typed YAML that round-trips through typed structs (T-B-08, T-B-09)
- [ ] All T-B-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
