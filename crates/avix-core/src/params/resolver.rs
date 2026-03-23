use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};
use crate::params::defaults::{AgentDefaults, DefaultsFile};
use crate::params::limits::{AgentLimits, LimitsFile};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Output: ResolvedConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEntrypoint {
    pub model_preference: String,
    pub min_context_tokens: u32,
    pub max_tool_chain: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMemory {
    pub episodic_enabled: bool,
    pub semantic_enabled: bool,
    pub preferences_enabled: bool,
    pub auto_inject_at_spawn: bool,
    pub auto_log_on_session_end: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSnapshot {
    pub enabled: bool,
    pub auto_snapshot_interval_sec: u32,
    pub restore_on_crash: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEnvironment {
    pub temperature: f32,
    pub timeout_sec: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPermissionsHint {
    pub owner: String,
    pub crew: String,
    pub world: String,
}

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

// ── Annotations: provenance tracking ─────────────────────────────────────────

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    pub value: serde_yaml::Value,
    pub source: AnnotationSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamped_from: Option<serde_yaml::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clamped_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Map of field path → Annotation. E.g. `"entrypoint.maxToolChain"` → Annotation { ... }
pub type Annotations = HashMap<String, Annotation>;

// ── ResolverInput ─────────────────────────────────────────────────────────────

pub struct LayeredDefaults {
    pub vfs_path: String,
    pub source: AnnotationSource,
    pub defaults: AgentDefaults,
}

pub struct LayeredLimits {
    pub vfs_path: String,
    pub limits: AgentLimits,
}

/// All inputs needed to produce a `ResolvedConfig`.
pub struct ResolverInput {
    pub system_defaults: AgentDefaults,
    pub system_defaults_path: String,
    pub system_limits: AgentLimits,
    pub system_limits_path: String,
    /// (vfs_path, limits) for each crew the user belongs to.
    pub crew_limits: Vec<LayeredLimits>,
    /// (vfs_path, defaults) for each crew, in membership order (later crews override earlier).
    pub crew_defaults: Vec<LayeredDefaults>,
    /// User defaults, if present.
    pub user_defaults: Option<LayeredDefaults>,
    /// User limits, if present.
    pub user_limits: Option<LayeredLimits>,
    /// Manifest overrides — highest priority defaults layer.
    pub manifest: AgentDefaults,
}

// ── ResolutionError ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ResolutionError {
    MissingRequired(String),
    HardViolation {
        field: String,
        value: String,
        allowed: Vec<String>,
        constrained_by: String,
    },
}

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRequired(field) => {
                write!(f, "resolution error: required field '{field}' has no value")
            }
            Self::HardViolation {
                field,
                value,
                allowed,
                constrained_by,
            } => write!(
                f,
                "resolution error: field '{field}' value '{value}' not in allowed set \
                 {:?} (constrained by {constrained_by})",
                allowed
            ),
        }
    }
}

impl std::error::Error for ResolutionError {}

// ── Internal working state ─────────────────────────────────────────────────────

/// Tracks the current winning value and its annotation for one field.
struct Field<T: Clone> {
    value: Option<T>,
    annotation: Option<Annotation>,
}

impl<T: Clone> Default for Field<T> {
    fn default() -> Self {
        Self {
            value: None,
            annotation: None,
        }
    }
}

impl<T: Clone + Into<serde_yaml::Value>> Field<T> {
    fn apply(&mut self, new_value: Option<T>, source: AnnotationSource, path: Option<&str>) {
        if let Some(v) = new_value {
            self.annotation = Some(Annotation {
                value: v.clone().into(),
                source,
                path: path.map(|s| s.to_string()),
                clamped_from: None,
                clamped_by: None,
                note: None,
            });
            self.value = Some(v);
        }
    }
}

// ── ParamResolver ─────────────────────────────────────────────────────────────

pub struct ParamResolver;

impl ParamResolver {
    /// Run the full resolution algorithm.
    /// Returns the resolved config and a provenance map.
    pub fn resolve(
        input: &ResolverInput,
    ) -> Result<(ResolvedConfig, Annotations), ResolutionError> {
        // Step 1: Compute effective limits (intersect all crew + user limits, system is ceiling)
        let effective_limits = Self::compute_effective_limits(input);

        // Step 2: Walk layers lowest-to-highest, applying each layer's values
        let mut model_preference = Field::<String>::default();
        let mut min_context_tokens = Field::<u32>::default();
        let mut max_tool_chain = Field::<u32>::default();
        let mut episodic_enabled = Field::<bool>::default();
        let mut semantic_enabled = Field::<bool>::default();
        let mut preferences_enabled = Field::<bool>::default();
        let mut auto_inject_at_spawn = Field::<bool>::default();
        let mut auto_log_on_session_end = Field::<bool>::default();
        let mut snap_enabled = Field::<bool>::default();
        let mut snap_interval = Field::<u32>::default();
        let mut restore_on_crash = Field::<bool>::default();
        let mut temperature = Field::<f32>::default();
        let mut timeout_sec = Field::<u32>::default();
        let mut perm_owner = Field::<String>::default();
        let mut perm_crew = Field::<String>::default();
        let mut perm_world = Field::<String>::default();

        macro_rules! apply_defaults {
            ($d:expr, $source:expr, $path:expr) => {
                if let Some(ep) = &$d.entrypoint {
                    model_preference.apply(ep.model_preference.clone(), $source.clone(), $path);
                    min_context_tokens.apply(ep.min_context_tokens, $source.clone(), $path);
                    max_tool_chain.apply(ep.max_tool_chain, $source.clone(), $path);
                }
                if let Some(mem) = &$d.memory {
                    episodic_enabled.apply(mem.episodic_enabled, $source.clone(), $path);
                    semantic_enabled.apply(mem.semantic_enabled, $source.clone(), $path);
                    preferences_enabled.apply(mem.preferences_enabled, $source.clone(), $path);
                    auto_inject_at_spawn.apply(mem.auto_inject_at_spawn, $source.clone(), $path);
                    auto_log_on_session_end
                        .apply(mem.auto_log_on_session_end, $source.clone(), $path);
                }
                if let Some(snap) = &$d.snapshot {
                    snap_enabled.apply(snap.enabled, $source.clone(), $path);
                    snap_interval.apply(snap.auto_snapshot_interval_sec, $source.clone(), $path);
                    restore_on_crash.apply(snap.restore_on_crash, $source.clone(), $path);
                }
                if let Some(env) = &$d.environment {
                    temperature.apply(env.temperature, $source.clone(), $path);
                    timeout_sec.apply(env.timeout_sec, $source.clone(), $path);
                }
                if let Some(ph) = &$d.permissions_hint {
                    perm_owner.apply(ph.owner.clone(), $source.clone(), $path);
                    perm_crew.apply(ph.crew.clone(), $source.clone(), $path);
                    perm_world.apply(ph.world.clone(), $source.clone(), $path);
                }
            };
        }

        // Layer 0: system defaults
        apply_defaults!(
            &input.system_defaults,
            AnnotationSource::SystemDefaults,
            Some(input.system_defaults_path.as_str())
        );

        // Layer 1: crew defaults (later entries in the vec override earlier)
        for ld in &input.crew_defaults {
            apply_defaults!(&ld.defaults, ld.source.clone(), Some(ld.vfs_path.as_str()));
        }

        // Layer 2: user defaults
        if let Some(ud) = &input.user_defaults {
            apply_defaults!(
                &ud.defaults,
                AnnotationSource::UserDefaults,
                Some(ud.vfs_path.as_str())
            );
        }

        // Layer 3: manifest (highest priority)
        apply_defaults!(&input.manifest, AnnotationSource::Manifest, None::<&str>);

        // Step 3: Clamp against effective limits
        // — entrypoint.maxToolChain (range)
        if let Some(lim) = effective_limits
            .entrypoint
            .as_ref()
            .and_then(|e| e.max_tool_chain.as_ref())
        {
            if let Some(v) = max_tool_chain.value {
                let clamped = lim.clamp(v as f64) as u32;
                if clamped != v {
                    let ann = max_tool_chain.annotation.as_mut().unwrap();
                    ann.clamped_from = Some(serde_yaml::Value::from(v));
                    ann.clamped_by = effective_limits_path_for_entrypoint(input);
                    ann.value = serde_yaml::Value::from(clamped);
                }
                max_tool_chain.value = Some(clamped);
            }
        }

        // — entrypoint.minContextTokens (range)
        if let Some(lim) = effective_limits
            .entrypoint
            .as_ref()
            .and_then(|e| e.min_context_tokens.as_ref())
        {
            if let Some(v) = min_context_tokens.value {
                let clamped = lim.clamp(v as f64) as u32;
                if clamped != v {
                    let ann = min_context_tokens.annotation.as_mut().unwrap();
                    ann.clamped_from = Some(serde_yaml::Value::from(v));
                    ann.clamped_by = effective_limits_path_for_entrypoint(input);
                    ann.value = serde_yaml::Value::from(clamped);
                }
                min_context_tokens.value = Some(clamped);
            }
        }

        // — entrypoint.modelPreference (enum) → HardViolation if not allowed
        if let Some(enum_c) = effective_limits
            .entrypoint
            .as_ref()
            .and_then(|e| e.model_preference.as_ref())
        {
            if let Some(ref v) = model_preference.value {
                if !enum_c.allows(v) {
                    return Err(ResolutionError::HardViolation {
                        field: "entrypoint.modelPreference".into(),
                        value: v.clone(),
                        allowed: enum_c.values.clone(),
                        constrained_by: effective_enum_limits_path(input),
                    });
                }
            }
        }

        // — environment.temperature (range)
        if let Some(lim) = effective_limits
            .environment
            .as_ref()
            .and_then(|e| e.temperature.as_ref())
        {
            if let Some(v) = temperature.value {
                let clamped = lim.clamp(v as f64) as f32;
                if (clamped - v).abs() > f32::EPSILON {
                    let ann = temperature.annotation.as_mut().unwrap();
                    ann.clamped_from = Some(serde_yaml::Value::from(v));
                    ann.clamped_by = effective_limits_path_for_environment(input);
                    ann.value = serde_yaml::Value::from(clamped);
                }
                temperature.value = Some(clamped);
            }
        }

        // — environment.timeoutSec (range)
        if let Some(lim) = effective_limits
            .environment
            .as_ref()
            .and_then(|e| e.timeout_sec.as_ref())
        {
            if let Some(v) = timeout_sec.value {
                let clamped = lim.clamp(v as f64) as u32;
                if clamped != v {
                    let ann = timeout_sec.annotation.as_mut().unwrap();
                    ann.clamped_from = Some(serde_yaml::Value::from(v));
                    ann.clamped_by = effective_limits_path_for_environment(input);
                    ann.value = serde_yaml::Value::from(clamped);
                }
                timeout_sec.value = Some(clamped);
            }
        }

        // Step 4: Build ResolvedConfig (error on missing required fields)
        let resolved = ResolvedConfig {
            entrypoint: ResolvedEntrypoint {
                model_preference: model_preference.value.ok_or_else(|| {
                    ResolutionError::MissingRequired("entrypoint.modelPreference".into())
                })?,
                min_context_tokens: min_context_tokens.value.unwrap_or(8_000),
                max_tool_chain: max_tool_chain.value.ok_or_else(|| {
                    ResolutionError::MissingRequired("entrypoint.maxToolChain".into())
                })?,
            },
            memory: ResolvedMemory {
                episodic_enabled: episodic_enabled.value.unwrap_or(true),
                semantic_enabled: semantic_enabled.value.unwrap_or(true),
                preferences_enabled: preferences_enabled.value.unwrap_or(true),
                auto_inject_at_spawn: auto_inject_at_spawn.value.unwrap_or(true),
                auto_log_on_session_end: auto_log_on_session_end.value.unwrap_or(false),
            },
            snapshot: ResolvedSnapshot {
                enabled: snap_enabled.value.unwrap_or(false),
                auto_snapshot_interval_sec: snap_interval.value.unwrap_or(0),
                restore_on_crash: restore_on_crash.value.unwrap_or(false),
            },
            environment: ResolvedEnvironment {
                temperature: temperature.value.ok_or_else(|| {
                    ResolutionError::MissingRequired("environment.temperature".into())
                })?,
                timeout_sec: timeout_sec.value.ok_or_else(|| {
                    ResolutionError::MissingRequired("environment.timeoutSec".into())
                })?,
            },
            permissions_hint: ResolvedPermissionsHint {
                owner: perm_owner.value.unwrap_or_else(|| "rw".into()),
                crew: perm_crew.value.unwrap_or_else(|| "r".into()),
                world: perm_world.value.unwrap_or_else(|| "r--".into()),
            },
        };

        // Step 5: Build annotations map
        let mut annotations = Annotations::new();
        macro_rules! emit_annotation {
            ($field:expr, $key:expr) => {
                if let Some(ann) = $field.annotation {
                    annotations.insert($key.to_string(), ann);
                }
            };
        }
        emit_annotation!(model_preference, "entrypoint.modelPreference");
        emit_annotation!(min_context_tokens, "entrypoint.minContextTokens");
        emit_annotation!(max_tool_chain, "entrypoint.maxToolChain");
        emit_annotation!(episodic_enabled, "memory.episodicEnabled");
        emit_annotation!(semantic_enabled, "memory.semanticEnabled");
        emit_annotation!(preferences_enabled, "memory.preferencesEnabled");
        emit_annotation!(auto_inject_at_spawn, "memory.autoInjectAtSpawn");
        emit_annotation!(auto_log_on_session_end, "memory.autoLogOnSessionEnd");
        emit_annotation!(snap_enabled, "snapshot.enabled");
        emit_annotation!(snap_interval, "snapshot.autoSnapshotIntervalSec");
        emit_annotation!(restore_on_crash, "snapshot.restoreOnCrash");
        emit_annotation!(temperature, "environment.temperature");
        emit_annotation!(timeout_sec, "environment.timeoutSec");
        emit_annotation!(perm_owner, "permissionsHint.owner");
        emit_annotation!(perm_crew, "permissionsHint.crew");
        emit_annotation!(perm_world, "permissionsHint.world");

        Ok((resolved, annotations))
    }

    fn compute_effective_limits(input: &ResolverInput) -> AgentLimits {
        let mut effective = input.system_limits.clone();
        for ll in &input.crew_limits {
            effective = effective.intersect(&ll.limits);
        }
        if let Some(ul) = &input.user_limits {
            effective = effective.intersect(&ul.limits);
        }
        effective
    }
}

// Helper: find the vfs_path of the limits file that most constrained entrypoint fields
fn effective_limits_path_for_entrypoint(input: &ResolverInput) -> Option<String> {
    // Last crew limit or user limit that has entrypoint constraints
    if let Some(ul) = &input.user_limits {
        if ul.limits.entrypoint.is_some() {
            return Some(ul.vfs_path.clone());
        }
    }
    for ll in input.crew_limits.iter().rev() {
        if ll.limits.entrypoint.is_some() {
            return Some(ll.vfs_path.clone());
        }
    }
    Some(input.system_limits_path.clone())
}

fn effective_limits_path_for_environment(input: &ResolverInput) -> Option<String> {
    if let Some(ul) = &input.user_limits {
        if ul.limits.environment.is_some() {
            return Some(ul.vfs_path.clone());
        }
    }
    for ll in input.crew_limits.iter().rev() {
        if ll.limits.environment.is_some() {
            return Some(ll.vfs_path.clone());
        }
    }
    Some(input.system_limits_path.clone())
}

fn effective_enum_limits_path(input: &ResolverInput) -> String {
    effective_limits_path_for_entrypoint(input).unwrap_or_else(|| input.system_limits_path.clone())
}

// ── ResolverInputLoader ───────────────────────────────────────────────────────

/// Loads `ResolverInput` from the VFS for a given user and their crew memberships.
pub struct ResolverInputLoader<'a> {
    vfs: &'a VfsRouter,
}

impl<'a> ResolverInputLoader<'a> {
    pub fn new(vfs: &'a VfsRouter) -> Self {
        Self { vfs }
    }

    pub async fn load(
        &self,
        username: &str,
        crew_names: &[String],
    ) -> Result<ResolverInput, AvixError> {
        let system_defaults_path = "/kernel/defaults/agent-manifest.yaml".to_string();
        let system_limits_path = "/kernel/limits/agent-manifest.yaml".to_string();

        let system_defaults = self
            .read_defaults(&system_defaults_path)
            .await?
            .ok_or_else(|| {
                AvixError::ConfigParse(
                    "system defaults missing: /kernel/defaults/agent-manifest.yaml".into(),
                )
            })?;

        let system_limits = self
            .read_limits(&system_limits_path)
            .await?
            .ok_or_else(|| {
                AvixError::ConfigParse(
                    "system limits missing: /kernel/limits/agent-manifest.yaml".into(),
                )
            })?;

        // Per-crew defaults and limits (missing files are silently skipped)
        let mut crew_defaults = Vec::new();
        let mut crew_limits = Vec::new();

        for crew in crew_names {
            let d_path = format!("/crews/{crew}/defaults.yaml");
            if let Some(d) = self.read_defaults(&d_path).await? {
                crew_defaults.push(LayeredDefaults {
                    vfs_path: d_path,
                    source: AnnotationSource::CrewDefaults,
                    defaults: d,
                });
            }

            let l_path = format!("/crews/{crew}/limits.yaml");
            if let Some(l) = self.read_limits(&l_path).await? {
                crew_limits.push(LayeredLimits {
                    vfs_path: l_path,
                    limits: l,
                });
            }
        }

        // User defaults and limits (missing files silently skipped)
        let user_defaults_path = format!("/users/{username}/defaults.yaml");
        let user_defaults =
            self.read_defaults(&user_defaults_path)
                .await?
                .map(|d| LayeredDefaults {
                    vfs_path: user_defaults_path,
                    source: AnnotationSource::UserDefaults,
                    defaults: d,
                });

        let user_limits_path = format!("/users/{username}/limits.yaml");
        let user_limits = self
            .read_limits(&user_limits_path)
            .await?
            .map(|l| LayeredLimits {
                vfs_path: user_limits_path,
                limits: l,
            });

        Ok(ResolverInput {
            system_defaults,
            system_defaults_path,
            system_limits,
            system_limits_path,
            crew_defaults,
            crew_limits,
            user_defaults,
            user_limits,
            manifest: AgentDefaults::default(),
        })
    }

    async fn read_defaults(&self, path: &str) -> Result<Option<AgentDefaults>, AvixError> {
        let vfs_path = VfsPath::parse(path)
            .map_err(|e| AvixError::ConfigParse(format!("invalid path {path}: {e}")))?;
        match self.vfs.read(&vfs_path).await {
            Ok(bytes) => {
                let text =
                    String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let file = DefaultsFile::from_str(&text)?;
                Ok(Some(file.as_agent_defaults()?))
            }
            Err(_) => Ok(None), // ENOENT → no defaults at this layer
        }
    }

    async fn read_limits(&self, path: &str) -> Result<Option<AgentLimits>, AvixError> {
        let vfs_path = VfsPath::parse(path)
            .map_err(|e| AvixError::ConfigParse(format!("invalid path {path}: {e}")))?;
        match self.vfs.read(&vfs_path).await {
            Ok(bytes) => {
                let text =
                    String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let file = LimitsFile::from_str(&text)?;
                Ok(Some(file.as_agent_limits()?))
            }
            Err(_) => Ok(None), // ENOENT → no limits at this layer
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::constraint::{EnumConstraint, RangeConstraint};
    use crate::params::defaults::{EntrypointDefaults, EnvironmentDefaults};
    use crate::params::limits::{EntrypointLimits, EnvironmentLimits};

    fn base_input() -> ResolverInput {
        ResolverInput {
            system_defaults: crate::params::defaults::system_agent_defaults(),
            system_defaults_path: "/kernel/defaults/agent-manifest.yaml".into(),
            system_limits: crate::params::limits::system_agent_limits(),
            system_limits_path: "/kernel/limits/agent-manifest.yaml".into(),
            crew_defaults: vec![],
            crew_limits: vec![],
            user_defaults: None,
            user_limits: None,
            manifest: AgentDefaults::default(),
        }
    }

    #[test]
    fn resolve_uses_system_defaults_when_no_overrides() {
        let input = base_input();
        let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 5);
        assert_eq!(
            annotations["entrypoint.maxToolChain"].source,
            AnnotationSource::SystemDefaults
        );
    }

    #[test]
    fn resolve_user_defaults_override_system() {
        let mut input = base_input();
        input.user_defaults = Some(LayeredDefaults {
            vfs_path: "/users/alice/defaults.yaml".into(),
            source: AnnotationSource::UserDefaults,
            defaults: AgentDefaults {
                entrypoint: Some(EntrypointDefaults {
                    max_tool_chain: Some(8),
                    ..Default::default()
                }),
                ..Default::default()
            },
        });
        let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 8);
        assert_eq!(
            annotations["entrypoint.maxToolChain"].source,
            AnnotationSource::UserDefaults
        );
    }

    #[test]
    fn resolve_manifest_overrides_user_defaults() {
        let mut input = base_input();
        input.user_defaults = Some(LayeredDefaults {
            vfs_path: "/users/alice/defaults.yaml".into(),
            source: AnnotationSource::UserDefaults,
            defaults: AgentDefaults {
                entrypoint: Some(EntrypointDefaults {
                    max_tool_chain: Some(8),
                    ..Default::default()
                }),
                ..Default::default()
            },
        });
        input.manifest = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(12),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 12);
    }

    #[test]
    fn resolve_clamps_to_system_limits_and_annotates() {
        let mut input = base_input();
        input.system_limits = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                max_tool_chain: Some(RangeConstraint {
                    min: Some(1.0),
                    max: Some(10.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        input.manifest = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(20),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (resolved, annotations) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 10);
        let ann = &annotations["entrypoint.maxToolChain"];
        assert_eq!(ann.clamped_from, Some(serde_yaml::Value::from(20u32)));
    }

    #[test]
    fn resolve_multi_crew_tightest_limit_applied() {
        let mut input = base_input();
        input.crew_limits = vec![
            LayeredLimits {
                vfs_path: "/crews/researchers/limits.yaml".into(),
                limits: AgentLimits {
                    entrypoint: Some(EntrypointLimits {
                        max_tool_chain: Some(RangeConstraint {
                            min: None,
                            max: Some(10.0),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
            LayeredLimits {
                vfs_path: "/crews/writers/limits.yaml".into(),
                limits: AgentLimits {
                    entrypoint: Some(EntrypointLimits {
                        max_tool_chain: Some(RangeConstraint {
                            min: None,
                            max: Some(5.0),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
        ];
        input.manifest = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 5);
    }

    #[test]
    fn resolve_enum_mismatch_returns_hard_violation() {
        let mut input = base_input();
        input.system_limits = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                model_preference: Some(EnumConstraint {
                    values: vec!["claude-sonnet-4".into(), "claude-haiku-4".into()],
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        // System defaults provide claude-sonnet-4, but manifest overrides to opus
        input.manifest = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                model_preference: Some("claude-opus-4".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = ParamResolver::resolve(&input);
        assert!(matches!(result, Err(ResolutionError::HardViolation { .. })));
    }

    #[test]
    fn resolve_crew_defaults_later_crew_wins() {
        let mut input = base_input();
        input.crew_defaults = vec![
            LayeredDefaults {
                vfs_path: "/crews/alpha/defaults.yaml".into(),
                source: AnnotationSource::CrewDefaults,
                defaults: AgentDefaults {
                    entrypoint: Some(EntrypointDefaults {
                        max_tool_chain: Some(3),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
            LayeredDefaults {
                vfs_path: "/crews/beta/defaults.yaml".into(),
                source: AnnotationSource::CrewDefaults,
                defaults: AgentDefaults {
                    entrypoint: Some(EntrypointDefaults {
                        max_tool_chain: Some(7),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            },
        ];
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.entrypoint.max_tool_chain, 7);
    }

    #[test]
    fn resolve_temperature_clamped_by_limit() {
        let mut input = base_input();
        input.system_limits = AgentLimits {
            environment: Some(EnvironmentLimits {
                temperature: Some(RangeConstraint {
                    min: Some(0.0),
                    max: Some(1.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        input.manifest = AgentDefaults {
            environment: Some(EnvironmentDefaults {
                temperature: Some(1.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        assert!((resolved.environment.temperature - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn resolve_permissions_hint_from_system_defaults() {
        let input = base_input();
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        assert_eq!(resolved.permissions_hint.owner, "rw");
        assert_eq!(resolved.permissions_hint.crew, "r");
    }

    #[tokio::test]
    async fn resolver_input_loader_reads_system_from_vfs() {
        use crate::bootstrap::phase1;
        use crate::memfs::VfsRouter;

        let vfs = VfsRouter::new();
        phase1::run(&vfs).await;

        let loader = ResolverInputLoader::new(&vfs);
        let input = loader.load("alice", &[]).await.unwrap();
        assert_eq!(
            input
                .system_defaults
                .entrypoint
                .as_ref()
                .unwrap()
                .max_tool_chain,
            Some(5)
        );
        assert!(input.user_defaults.is_none());
        assert!(input.crew_defaults.is_empty());
    }

    #[tokio::test]
    async fn resolver_input_loader_reads_user_defaults_from_vfs() {
        use crate::bootstrap::phase1;
        use crate::memfs::VfsRouter;
        use crate::params::defaults::{DefaultsFile, DefaultsLayer};
        use crate::params::limits::{LimitsFile, LimitsLayer};

        let vfs = VfsRouter::new();
        phase1::run(&vfs).await;

        // Write user defaults
        let user_d = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        };
        let yaml =
            DefaultsFile::from_agent_defaults(DefaultsLayer::User, Some("alice".into()), &user_d)
                .unwrap();
        vfs.write(
            &VfsPath::parse("/users/alice/defaults.yaml").unwrap(),
            yaml.into_bytes(),
        )
        .await
        .unwrap();

        // Write crew limits
        let crew_l = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                max_tool_chain: Some(RangeConstraint {
                    min: None,
                    max: Some(6.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let lim_yaml =
            LimitsFile::from_agent_limits(LimitsLayer::Crew, Some("research".into()), &crew_l)
                .unwrap();
        vfs.write(
            &VfsPath::parse("/crews/research/limits.yaml").unwrap(),
            lim_yaml.into_bytes(),
        )
        .await
        .unwrap();

        let loader = ResolverInputLoader::new(&vfs);
        let input = loader
            .load("alice", &["research".to_string()])
            .await
            .unwrap();
        assert!(input.user_defaults.is_some());
        assert_eq!(input.crew_limits.len(), 1);

        // Full resolution
        let (resolved, _) = ParamResolver::resolve(&input).unwrap();
        // user wants 7, crew caps at 6 → clamped to 6
        assert_eq!(resolved.entrypoint.max_tool_chain, 6);
    }
}
