use crate::error::AvixError;
use serde::{Deserialize, Serialize};

// ── Sub-structs ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EntrypointDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_context_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_chain: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodic_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferences_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_inject_at_spawn: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_log_on_session_end: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_snapshot_interval_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_on_crash: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvironmentDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_sec: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsHintDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crew: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

/// The `defaults:` block for `target: agent-manifest`.
/// All fields are `Option` — `None` means "not set at this layer".
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<EntrypointDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions_hint: Option<PermissionsHintDefaults>,
}

impl AgentDefaults {
    /// Apply `other` on top of `self`: fields set in `other` override `self`.
    /// Fields absent in `other` (None) keep the value from `self`.
    pub fn merge_over(&self, other: &AgentDefaults) -> AgentDefaults {
        AgentDefaults {
            entrypoint: merge_option(
                self.entrypoint.as_ref(),
                other.entrypoint.as_ref(),
                merge_entrypoint,
            ),
            memory: merge_option(self.memory.as_ref(), other.memory.as_ref(), merge_memory),
            snapshot: merge_option(
                self.snapshot.as_ref(),
                other.snapshot.as_ref(),
                merge_snapshot,
            ),
            environment: merge_option(
                self.environment.as_ref(),
                other.environment.as_ref(),
                merge_environment,
            ),
            permissions_hint: merge_option(
                self.permissions_hint.as_ref(),
                other.permissions_hint.as_ref(),
                merge_permissions_hint,
            ),
        }
    }
}

fn merge_option<T: Clone>(
    base: Option<&T>,
    over: Option<&T>,
    merge_fn: impl Fn(&T, &T) -> T,
) -> Option<T> {
    match (base, over) {
        (Some(b), Some(o)) => Some(merge_fn(b, o)),
        (Some(b), None) => Some(b.clone()),
        (None, Some(o)) => Some(o.clone()),
        (None, None) => None,
    }
}

fn merge_entrypoint(base: &EntrypointDefaults, over: &EntrypointDefaults) -> EntrypointDefaults {
    EntrypointDefaults {
        model_preference: over
            .model_preference
            .clone()
            .or(base.model_preference.clone()),
        min_context_tokens: over.min_context_tokens.or(base.min_context_tokens),
        max_tool_chain: over.max_tool_chain.or(base.max_tool_chain),
    }
}

fn merge_memory(base: &MemoryDefaults, over: &MemoryDefaults) -> MemoryDefaults {
    MemoryDefaults {
        episodic_enabled: over.episodic_enabled.or(base.episodic_enabled),
        semantic_enabled: over.semantic_enabled.or(base.semantic_enabled),
        preferences_enabled: over.preferences_enabled.or(base.preferences_enabled),
        auto_inject_at_spawn: over.auto_inject_at_spawn.or(base.auto_inject_at_spawn),
        auto_log_on_session_end: over.auto_log_on_session_end.or(base.auto_log_on_session_end),
    }
}

fn merge_snapshot(base: &SnapshotDefaults, over: &SnapshotDefaults) -> SnapshotDefaults {
    SnapshotDefaults {
        enabled: over.enabled.or(base.enabled),
        auto_snapshot_interval_sec: over
            .auto_snapshot_interval_sec
            .or(base.auto_snapshot_interval_sec),
        restore_on_crash: over.restore_on_crash.or(base.restore_on_crash),
    }
}

fn merge_environment(
    base: &EnvironmentDefaults,
    over: &EnvironmentDefaults,
) -> EnvironmentDefaults {
    EnvironmentDefaults {
        temperature: over.temperature.or(base.temperature),
        timeout_sec: over.timeout_sec.or(base.timeout_sec),
    }
}

fn merge_permissions_hint(
    base: &PermissionsHintDefaults,
    over: &PermissionsHintDefaults,
) -> PermissionsHintDefaults {
    PermissionsHintDefaults {
        owner: over.owner.clone().or(base.owner.clone()),
        crew: over.crew.clone().or(base.crew.clone()),
        world: over.world.clone().or(base.world.clone()),
    }
}

// ── DefaultsFile envelope ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DefaultsLayer {
    System,
    User,
    Crew,
    Service,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultsMetadata {
    pub target: String,
    pub layer: DefaultsLayer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub updated_at: String,
}

/// Full Defaults file envelope (`kind: Defaults`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: DefaultsMetadata,
    /// Raw YAML value of the `defaults:` block; parse with `as_agent_defaults()`.
    pub defaults: serde_yaml::Value,
}

impl DefaultsFile {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Parse `defaults:` block as `AgentDefaults`.
    pub fn as_agent_defaults(&self) -> Result<AgentDefaults, AvixError> {
        serde_yaml::from_value(self.defaults.clone())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Serialise an `AgentDefaults` into a `DefaultsFile` YAML string.
    pub fn from_agent_defaults(
        layer: DefaultsLayer,
        owner: Option<String>,
        defaults: &AgentDefaults,
    ) -> Result<String, AvixError> {
        let file = DefaultsFile {
            api_version: "avix/v1".into(),
            kind: "Defaults".into(),
            metadata: DefaultsMetadata {
                target: "agent-manifest".into(),
                layer,
                owner,
                updated_at: String::new(),
            },
            defaults: serde_yaml::to_value(defaults)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        };
        serde_yaml::to_string(&file).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }
}

// ── System-level defaults (compiled in) ──────────────────────────────────────

/// Returns the compiled-in system-level `AgentDefaults`.
pub fn system_agent_defaults() -> AgentDefaults {
    AgentDefaults {
        entrypoint: Some(EntrypointDefaults {
            model_preference: Some("claude-sonnet-4".into()),
            min_context_tokens: Some(8_000),
            max_tool_chain: Some(5),
        }),
        memory: Some(MemoryDefaults {
            episodic_enabled: Some(true),
            semantic_enabled: Some(true),
            preferences_enabled: Some(true),
            auto_inject_at_spawn: Some(true),
            auto_log_on_session_end: Some(false),
        }),
        snapshot: Some(SnapshotDefaults {
            enabled: Some(false),
            auto_snapshot_interval_sec: Some(0),
            restore_on_crash: Some(false),
        }),
        environment: Some(EnvironmentDefaults {
            temperature: Some(0.7),
            timeout_sec: Some(300),
        }),
        permissions_hint: Some(PermissionsHintDefaults {
            owner: Some("rw".into()),
            crew: Some("r".into()),
            world: Some("r--".into()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_example_yaml() -> &'static str {
        r#"
apiVersion: avix/v1
kind: Defaults
metadata:
  target: agent-manifest
  layer: system
  owner: null
  updatedAt: "2026-03-15T07:38:00-05:00"
defaults:
  entrypoint:
    modelPreference: claude-sonnet-4
    minContextTokens: 8000
    maxToolChain: 5
  environment:
    temperature: 0.7
    timeoutSec: 300
"#
    }

    #[test]
    fn defaults_file_parses_spec_example() {
        let file = DefaultsFile::from_str(spec_example_yaml()).unwrap();
        assert_eq!(file.metadata.target, "agent-manifest");
        assert_eq!(file.metadata.layer, DefaultsLayer::System);
        let d = file.as_agent_defaults().unwrap();
        let ep = d.entrypoint.unwrap();
        assert_eq!(ep.max_tool_chain, Some(5));
        let env = d.environment.unwrap();
        assert!((env.temperature.unwrap() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn defaults_file_user_layer_parses() {
        let yaml = r#"
apiVersion: avix/v1
kind: Defaults
metadata:
  target: agent-manifest
  layer: user
  owner: alice
  updatedAt: "2026-03-15T09:00:00-05:00"
defaults:
  entrypoint:
    maxToolChain: 8
    modelPreference: claude-opus-4
  snapshot:
    enabled: true
    autoSnapshotIntervalSec: 300
    restoreOnCrash: true
"#;
        let file = DefaultsFile::from_str(yaml).unwrap();
        assert_eq!(file.metadata.layer, DefaultsLayer::User);
        assert_eq!(file.metadata.owner, Some("alice".into()));
        let d = file.as_agent_defaults().unwrap();
        assert_eq!(d.entrypoint.unwrap().max_tool_chain, Some(8));
        let snap = d.snapshot.unwrap();
        assert_eq!(snap.enabled, Some(true));
    }

    #[test]
    fn agent_defaults_merge_over_overrides_fields() {
        let base = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(5),
                model_preference: Some("sonnet".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let over = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };
        let merged = base.merge_over(&over);
        let ep = merged.entrypoint.unwrap();
        assert_eq!(ep.max_tool_chain, Some(8));
        // model_preference preserved from base when not set in over
        assert_eq!(ep.model_preference, Some("sonnet".into()));
    }

    #[test]
    fn system_agent_defaults_has_expected_values() {
        let d = system_agent_defaults();
        assert_eq!(d.entrypoint.as_ref().unwrap().max_tool_chain, Some(5));
        assert!((d.environment.as_ref().unwrap().temperature.unwrap() - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn defaults_file_round_trips_from_agent_defaults() {
        let d = system_agent_defaults();
        let yaml = DefaultsFile::from_agent_defaults(DefaultsLayer::System, None, &d).unwrap();
        let parsed = DefaultsFile::from_str(&yaml).unwrap();
        let d2 = parsed.as_agent_defaults().unwrap();
        assert_eq!(d, d2);
    }
}
