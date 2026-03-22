# Param Gap C — Parameter Resolution Engine

> **Status:** Not started
> **Priority:** High — required before resolved-at-spawn (Gap D) and avix resolve CLI (Gap E)
> **Depends on:** Gap B (typed Defaults and Limits structs)
> **Affects:** new `avix-core/src/params/resolver.rs`

---

## Problem

There is no code that merges the layered defaults/limits system into a single `Resolved`
output. The spec (`docs/spec/param-resolved.md`) defines a precise resolution algorithm:

1. Start with system defaults (`/kernel/defaults/agent-manifest.yaml`)
2. Apply crew defaults (`/crews/<crew>/defaults.yaml`) — each crew in the user's membership
3. Apply user defaults (`/users/<u>/defaults.yaml`) — overrides crew defaults
4. Apply manifest values (agent author's explicit settings) — highest priority
5. Clamp every value against the tightest effective limits (crew limits intersected, then
   system limits applied as ceiling)
6. Track provenance — record which file each winning value came from

The `resolved.yaml` written by `RuntimeExecutor` today uses hard-coded literals and skips
all of this. The `AgentDefaults` and `AgentLimits` types (Gap B) are a prerequisite.

---

## What Needs to Be Built

### 1. `ResolvedConfig` output type (`params/resolver.rs`)

```rust
/// The final, merged configuration an agent actually runs with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedConfig {
    pub entrypoint: ResolvedEntrypoint,
    pub memory: ResolvedMemory,
    pub snapshot: ResolvedSnapshot,
    pub environment: ResolvedEnvironment,
    pub permissions_hint: ResolvedPermissionsHint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEntrypoint {
    pub model_preference: String,
    pub min_context_tokens: u32,
    pub max_tool_chain: u32,
}

// ... similarly for ResolvedMemory, ResolvedSnapshot, ResolvedEnvironment, ResolvedPermissionsHint
```

### 2. `Annotation` provenance record

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    pub value: serde_yaml::Value,
    pub source: AnnotationSource,
    pub path: Option<String>,        // VFS path of the file that provided this value
    pub clamped_from: Option<serde_yaml::Value>,  // original value before clamping
    pub clamped_by: Option<String>,  // VFS path of the limits file that clamped it
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AnnotationSource {
    SystemDefaults,
    SystemLimits,
    UserDefaults,
    UserLimits,
    CrewDefaults,
    CrewLimits,
    Manifest,
}

/// Map of field path → Annotation. E.g. "entrypoint.maxToolChain" → Annotation { ... }
pub type Annotations = HashMap<String, Annotation>;
```

### 3. `ResolverInput` — all inputs to the engine

```rust
pub struct ResolverInput {
    /// System defaults from /kernel/defaults/agent-manifest.yaml
    pub system_defaults: AgentDefaults,

    /// System limits from /kernel/limits/agent-manifest.yaml
    pub system_limits: AgentLimits,

    /// Per-crew defaults, in membership order.
    /// Each entry: (crew_name, vfs_path, AgentDefaults)
    pub crew_defaults: Vec<(String, String, AgentDefaults)>,

    /// Per-crew limits, in membership order.
    /// Each entry: (crew_name, vfs_path, AgentLimits)
    pub crew_limits: Vec<(String, String, AgentLimits)>,

    /// User defaults from /users/<u>/defaults.yaml (None if file absent)
    pub user_defaults: Option<(String, AgentDefaults)>,  // (vfs_path, defaults)

    /// User limits from /users/<u>/limits.yaml (None if file absent)
    pub user_limits: Option<(String, AgentLimits)>,      // (vfs_path, limits)

    /// Manifest overrides from AgentManifest.spec (None fields = not set)
    pub manifest: AgentDefaults,
}
```

### 4. `ParamResolver` — the merge engine

```rust
pub struct ParamResolver;

impl ParamResolver {
    /// Run the full resolution algorithm. Returns resolved config and provenance map.
    pub fn resolve(input: &ResolverInput) -> Result<(ResolvedConfig, Annotations), ResolutionError> {
        // Step 1: Compute effective limits (intersect all crew limits, then system limits as ceiling)
        let effective_limits = Self::compute_effective_limits(input);

        // Step 2: Walk layers lowest-to-highest, applying each layer's values
        //   layer 0: system_defaults (AnnotationSource::SystemDefaults)
        //   layer 1: crew_defaults (each crew, AnnotationSource::CrewDefaults)
        //   layer 2: user_defaults (AnnotationSource::UserDefaults)
        //   layer 3: manifest (AnnotationSource::Manifest)
        let mut working = WorkingValues::from_defaults(&input.system_defaults, &input.system_defaults_path);
        for (crew, path, crew_d) in &input.crew_defaults {
            working.apply_defaults(crew_d, AnnotationSource::CrewDefaults, path);
        }
        if let Some((path, user_d)) = &input.user_defaults {
            working.apply_defaults(user_d, AnnotationSource::UserDefaults, path);
        }
        working.apply_defaults(&input.manifest, AnnotationSource::Manifest, "manifest");

        // Step 3: Clamp against effective limits; record clamping in annotations
        Self::clamp_and_annotate(&mut working, &effective_limits)?;

        // Step 4: Build ResolvedConfig from working values
        let resolved = working.into_resolved()?;
        let annotations = working.into_annotations();

        Ok((resolved, annotations))
    }

    fn compute_effective_limits(input: &ResolverInput) -> AgentLimits {
        // Start from system limits
        let mut effective = input.system_limits.clone();
        // Intersect each crew's limits (tightest wins per field)
        for (_, _, crew_l) in &input.crew_limits {
            effective = effective.intersect(crew_l);
        }
        // Intersect user's limits
        if let Some((_, user_l)) = &input.user_limits {
            effective = effective.intersect(user_l);
        }
        effective
    }
}

pub enum ResolutionError {
    /// A required field has no value at any layer.
    MissingRequired(String),
    /// A manifest value violates limits that cannot be clamped (e.g. enum mismatch).
    HardViolation { field: String, value: String, constraint: Constraint },
}
```

**Clamping rules:**
- `RangeConstraint`: clamp value to `[min, max]` — record `clamped_from` if changed
- `EnumConstraint`: if value not in allowed set, return `HardViolation` (cannot guess intent)
- `SetConstraint`: remove disallowed items from lists silently; annotate removals in `note`
- `BoolConstraint { value: Some(v) }`: force to `v`; annotate if different from requested
- `BoolConstraint { value: None }`: no constraint, pass through

### 5. `ResolverInputLoader` — reads inputs from VFS

```rust
pub struct ResolverInputLoader<'vfs> {
    vfs: &'vfs MemFs,
}

impl<'vfs> ResolverInputLoader<'vfs> {
    pub async fn load(
        &self,
        username: &str,
        crew_names: &[String],
    ) -> Result<ResolverInput, AvixError> {
        // Read /kernel/defaults/agent-manifest.yaml
        // Read /kernel/limits/agent-manifest.yaml
        // For each crew: read /crews/<crew>/defaults.yaml (if exists)
        // For each crew: read /crews/<crew>/limits.yaml (if exists)
        // Read /users/<username>/defaults.yaml (if exists)
        // Read /users/<username>/limits.yaml (if exists)
        // Return ResolverInput
    }
}
```

Missing files at user/crew layers are silently skipped (no file = no overrides at that
layer). Missing system defaults or limits files are a fatal error.

---

## Resolution Algorithm (Field-by-Field)

For each field `F` in `AgentDefaults`/`AgentLimits`:

```
effective_value(F):
  candidates = [
    system_defaults.F,      # lowest priority
    crew_defaults[*].F,     # later crews win over earlier
    user_defaults.F,
    manifest.F,             # highest priority
  ]
  raw = first non-None value from candidates reversed (manifest wins)
  if raw is None: error MissingRequired if field is required, else use built-in fallback
  clamped = apply effective_limits.F to raw
  annotation.source = layer that provided raw
  annotation.clamped_from = raw if clamped != raw
  return clamped
```

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/param_resolver.rs` (new file).

```rust
// T-C-01: System defaults used when no user/crew/manifest values set
#[test]
fn resolve_uses_system_defaults_when_no_overrides() {
    let input = ResolverInput {
        system_defaults: AgentDefaults {
            entrypoint: EntrypointDefaults { max_tool_chain: Some(5), ..Default::default() },
            ..Default::default()
        },
        system_limits: AgentLimits::default(),
        crew_defaults: vec![],
        crew_limits: vec![],
        user_defaults: None,
        user_limits: None,
        manifest: AgentDefaults::default(),
    };
    let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 5);
    assert_eq!(annotations["entrypoint.maxToolChain"].source, AnnotationSource::SystemDefaults);
}

// T-C-02: User defaults override system defaults
#[test]
fn resolve_user_defaults_override_system() {
    let mut input = make_base_input();
    input.user_defaults = Some((
        "/users/alice/defaults.yaml".into(),
        AgentDefaults {
            entrypoint: EntrypointDefaults { max_tool_chain: Some(8), ..Default::default() },
            ..Default::default()
        },
    ));
    let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 8);
    assert_eq!(annotations["entrypoint.maxToolChain"].source, AnnotationSource::UserDefaults);
}

// T-C-03: Manifest value overrides user defaults
#[test]
fn resolve_manifest_overrides_user_defaults() {
    let mut input = make_base_input();
    input.user_defaults = Some(("/users/alice/defaults.yaml".into(), AgentDefaults {
        entrypoint: EntrypointDefaults { max_tool_chain: Some(8), ..Default::default() },
        ..Default::default()
    }));
    input.manifest = AgentDefaults {
        entrypoint: EntrypointDefaults { max_tool_chain: Some(12), ..Default::default() },
        ..Default::default()
    };
    let (resolved, _) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 12);
}

// T-C-04: Value clamped by system limits — clamped_from annotated
#[test]
fn resolve_clamps_to_system_limits_and_annotates() {
    let mut input = make_base_input();
    input.system_limits = AgentLimits {
        entrypoint: EntrypointLimits {
            max_tool_chain: Some(RangeConstraint { min: Some(1.0), max: Some(10.0) }),
            ..Default::default()
        },
        ..Default::default()
    };
    input.manifest = AgentDefaults {
        entrypoint: EntrypointDefaults { max_tool_chain: Some(20), ..Default::default() },
        ..Default::default()
    };
    let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 10);
    let ann = &annotations["entrypoint.maxToolChain"];
    assert_eq!(ann.clamped_from, Some(serde_yaml::Value::from(20)));
}

// T-C-05: Multi-crew limits intersected (tightest wins)
#[test]
fn resolve_multi_crew_tightest_limit_applied() {
    let mut input = make_base_input();
    input.crew_limits = vec![
        ("researchers".into(), "/crews/researchers/limits.yaml".into(), AgentLimits {
            entrypoint: EntrypointLimits {
                max_tool_chain: Some(RangeConstraint { min: None, max: Some(10.0) }),
                ..Default::default()
            },
            ..Default::default()
        }),
        ("writers".into(), "/crews/writers/limits.yaml".into(), AgentLimits {
            entrypoint: EntrypointLimits {
                max_tool_chain: Some(RangeConstraint { min: None, max: Some(5.0) }),
                ..Default::default()
            },
            ..Default::default()
        }),
    ];
    input.manifest = AgentDefaults {
        entrypoint: EntrypointDefaults { max_tool_chain: Some(8), ..Default::default() },
        ..Default::default()
    };
    let (resolved, _) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 5);
}

// T-C-06: HardViolation returned for enum constraint mismatch
#[test]
fn resolve_enum_mismatch_returns_hard_violation() {
    let mut input = make_base_input();
    input.system_limits = AgentLimits {
        entrypoint: EntrypointLimits {
            model_preference: Some(EnumConstraint {
                values: vec!["claude-sonnet-4".into(), "claude-haiku-4".into()],
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    input.manifest = AgentDefaults {
        entrypoint: EntrypointDefaults {
            model_preference: Some("claude-opus-4".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = ParamResolver::resolve(&input);
    assert!(matches!(result, Err(ResolutionError::HardViolation { .. })));
}

// T-C-07: Crew defaults applied in order (last crew wins over earlier)
#[test]
fn resolve_crew_defaults_later_crew_wins() {
    let mut input = make_base_input();
    input.crew_defaults = vec![
        ("alpha".into(), "/crews/alpha/defaults.yaml".into(), AgentDefaults {
            entrypoint: EntrypointDefaults { max_tool_chain: Some(3), ..Default::default() },
            ..Default::default()
        }),
        ("beta".into(), "/crews/beta/defaults.yaml".into(), AgentDefaults {
            entrypoint: EntrypointDefaults { max_tool_chain: Some(7), ..Default::default() },
            ..Default::default()
        }),
    ];
    let (resolved, _) = ParamResolver::resolve(&input).unwrap();
    assert_eq!(resolved.entrypoint.max_tool_chain, 7);
}

// T-C-08: ResolverInputLoader reads from VFS correctly
#[tokio::test]
async fn resolver_input_loader_reads_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let vfs = setup_vfs_with_defaults_and_limits(&dir).await;
    let loader = ResolverInputLoader::new(&vfs);
    let input = loader.load("alice", &["researchers".into()]).await.unwrap();
    assert!(input.user_defaults.is_some());
    assert_eq!(input.crew_defaults.len(), 1);
}
```

---

## Implementation Notes

- The `WorkingValues` internal type (not public) tracks both the current value and its
  annotation for each field. Use a `HashMap<String, (serde_yaml::Value, Annotation)>`.
- Keep `ParamResolver::resolve` pure — it takes a `&ResolverInput` and returns results
  with no I/O. All I/O is in `ResolverInputLoader`. This makes the resolution logic
  trivially testable.
- Required fields (those that must have a value after resolution): `entrypoint.model_preference`,
  `entrypoint.max_tool_chain`, `environment.temperature`, `environment.timeout_sec`.
  All others may be `None` in the output if absent from all layers.
- Annotations are generated for every field that has a value, whether clamped or not.
- Do not expose `WorkingValues` in the public API — it is an implementation detail.

---

## Success Criteria

- [ ] `ParamResolver::resolve` correctly applies all 4 layers (system → crew → user → manifest)
- [ ] Limits intersection produces tightest constraint (T-C-05)
- [ ] Value clamping records `clamped_from` in annotation (T-C-04)
- [ ] Enum hard violations are returned as `ResolutionError::HardViolation` (T-C-06)
- [ ] `ResolverInputLoader` reads defaults and limits from VFS for user and crews (T-C-08)
- [ ] All T-C-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
