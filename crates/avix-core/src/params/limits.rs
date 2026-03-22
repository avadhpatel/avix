use crate::error::AvixError;
use crate::params::constraint::{BoolConstraint, EnumConstraint, RangeConstraint, SetConstraint};
use crate::params::defaults::AgentDefaults;
use serde::{Deserialize, Serialize};

// ── Sub-structs ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EntrypointLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_preference: Option<EnumConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_context_tokens: Option<RangeConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_chain: Option<RangeConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<SetConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<SetConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_context: Option<EnumConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_store_access: Option<EnumConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<BoolConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_snapshot_interval_sec: Option<RangeConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EnvironmentLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<RangeConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_sec: Option<RangeConstraint>,
}

/// Full agent-manifest limits; all fields are `Option` — `None` means unconstrained at this layer.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<EntrypointLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotLimits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentLimits>,
}

/// A single constraint violation found by `AgentLimits::check_defaults`.
#[derive(Debug, Clone)]
pub struct LimitViolation {
    pub field: String,
    pub value: String,
}

impl AgentLimits {
    /// Returns the tightest combination of `self` and `other` for every field.
    /// `None` on either side means "unconstrained at that layer" — the other side wins.
    pub fn intersect(&self, other: &AgentLimits) -> AgentLimits {
        AgentLimits {
            entrypoint: intersect_option(
                self.entrypoint.as_ref(),
                other.entrypoint.as_ref(),
                intersect_entrypoint,
            ),
            tools: intersect_option(self.tools.as_ref(), other.tools.as_ref(), intersect_tools),
            memory: intersect_option(
                self.memory.as_ref(),
                other.memory.as_ref(),
                intersect_memory,
            ),
            snapshot: intersect_option(
                self.snapshot.as_ref(),
                other.snapshot.as_ref(),
                intersect_snapshot,
            ),
            environment: intersect_option(
                self.environment.as_ref(),
                other.environment.as_ref(),
                intersect_environment,
            ),
        }
    }

    /// Check that every set field in `defaults` satisfies the corresponding constraint.
    /// Returns `Err` with a list of violations; `Ok(())` if all constraints pass.
    pub fn check_defaults(&self, defaults: &AgentDefaults) -> Result<(), Vec<LimitViolation>> {
        let mut violations = Vec::new();

        if let (Some(lim), Some(def)) = (&self.entrypoint, &defaults.entrypoint) {
            if let (Some(c), Some(v)) = (&lim.max_tool_chain, def.max_tool_chain) {
                if !c.allows(v as f64) {
                    violations.push(LimitViolation {
                        field: "entrypoint.maxToolChain".into(),
                        value: v.to_string(),
                    });
                }
            }
            if let (Some(c), Some(v)) = (&lim.min_context_tokens, def.min_context_tokens) {
                if !c.allows(v as f64) {
                    violations.push(LimitViolation {
                        field: "entrypoint.minContextTokens".into(),
                        value: v.to_string(),
                    });
                }
            }
            if let (Some(c), Some(v)) = (&lim.model_preference, &def.model_preference) {
                if !c.allows(v) {
                    violations.push(LimitViolation {
                        field: "entrypoint.modelPreference".into(),
                        value: v.clone(),
                    });
                }
            }
        }

        if let (Some(lim), Some(def)) = (&self.environment, &defaults.environment) {
            if let (Some(c), Some(v)) = (&lim.temperature, def.temperature) {
                if !c.allows(v as f64) {
                    violations.push(LimitViolation {
                        field: "environment.temperature".into(),
                        value: v.to_string(),
                    });
                }
            }
            if let (Some(c), Some(v)) = (&lim.timeout_sec, def.timeout_sec) {
                if !c.allows(v as f64) {
                    violations.push(LimitViolation {
                        field: "environment.timeoutSec".into(),
                        value: v.to_string(),
                    });
                }
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

fn intersect_option<T: Clone>(a: Option<&T>, b: Option<&T>, f: impl Fn(&T, &T) -> T) -> Option<T> {
    match (a, b) {
        (Some(x), Some(y)) => Some(f(x, y)),
        (Some(x), None) => Some(x.clone()),
        (None, Some(y)) => Some(y.clone()),
        (None, None) => None,
    }
}

fn intersect_entrypoint(a: &EntrypointLimits, b: &EntrypointLimits) -> EntrypointLimits {
    EntrypointLimits {
        model_preference: intersect_option(
            a.model_preference.as_ref(),
            b.model_preference.as_ref(),
            EnumConstraint::intersect,
        ),
        min_context_tokens: intersect_option(
            a.min_context_tokens.as_ref(),
            b.min_context_tokens.as_ref(),
            RangeConstraint::intersect,
        ),
        max_tool_chain: intersect_option(
            a.max_tool_chain.as_ref(),
            b.max_tool_chain.as_ref(),
            RangeConstraint::intersect,
        ),
    }
}

fn intersect_tools(a: &ToolsLimits, b: &ToolsLimits) -> ToolsLimits {
    ToolsLimits {
        required: intersect_option(
            a.required.as_ref(),
            b.required.as_ref(),
            SetConstraint::intersect,
        ),
        optional: intersect_option(
            a.optional.as_ref(),
            b.optional.as_ref(),
            SetConstraint::intersect,
        ),
    }
}

fn intersect_memory(a: &MemoryLimits, b: &MemoryLimits) -> MemoryLimits {
    MemoryLimits {
        working_context: intersect_option(
            a.working_context.as_ref(),
            b.working_context.as_ref(),
            EnumConstraint::intersect,
        ),
        semantic_store_access: intersect_option(
            a.semantic_store_access.as_ref(),
            b.semantic_store_access.as_ref(),
            EnumConstraint::intersect,
        ),
    }
}

fn intersect_snapshot(a: &SnapshotLimits, b: &SnapshotLimits) -> SnapshotLimits {
    SnapshotLimits {
        enabled: intersect_option(
            a.enabled.as_ref(),
            b.enabled.as_ref(),
            BoolConstraint::intersect,
        ),
        auto_snapshot_interval_sec: intersect_option(
            a.auto_snapshot_interval_sec.as_ref(),
            b.auto_snapshot_interval_sec.as_ref(),
            RangeConstraint::intersect,
        ),
    }
}

fn intersect_environment(a: &EnvironmentLimits, b: &EnvironmentLimits) -> EnvironmentLimits {
    EnvironmentLimits {
        temperature: intersect_option(
            a.temperature.as_ref(),
            b.temperature.as_ref(),
            RangeConstraint::intersect,
        ),
        timeout_sec: intersect_option(
            a.timeout_sec.as_ref(),
            b.timeout_sec.as_ref(),
            RangeConstraint::intersect,
        ),
    }
}

// ── LimitsFile envelope ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LimitsLayer {
    System,
    User,
    Crew,
    Service,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsMetadata {
    pub target: String,
    pub layer: LimitsLayer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub reason: String,
}

/// Full Limits file envelope (`kind: Limits`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: LimitsMetadata,
    /// Raw YAML value of the `limits:` block; parse with `as_agent_limits()`.
    pub limits: serde_yaml::Value,
}

impl LimitsFile {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Parse `limits:` block as `AgentLimits`.
    pub fn as_agent_limits(&self) -> Result<AgentLimits, AvixError> {
        serde_yaml::from_value(self.limits.clone())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Serialise an `AgentLimits` into a `LimitsFile` YAML string.
    pub fn from_agent_limits(
        layer: LimitsLayer,
        owner: Option<String>,
        limits: &AgentLimits,
    ) -> Result<String, AvixError> {
        let file = LimitsFile {
            api_version: "avix/v1".into(),
            kind: "Limits".into(),
            metadata: LimitsMetadata {
                target: "agent-manifest".into(),
                layer,
                owner,
                updated_at: String::new(),
                updated_by: "kernel".into(),
                reason: "boot".into(),
            },
            limits: serde_yaml::to_value(limits)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        };
        serde_yaml::to_string(&file).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }
}

// ── System-level limits (compiled in) ────────────────────────────────────────

/// Returns the compiled-in system-level `AgentLimits`.
pub fn system_agent_limits() -> AgentLimits {
    AgentLimits {
        entrypoint: Some(EntrypointLimits {
            model_preference: None, // any model allowed at system level
            min_context_tokens: Some(RangeConstraint {
                min: Some(1_000.0),
                max: Some(200_000.0),
            }),
            max_tool_chain: Some(RangeConstraint {
                min: Some(1.0),
                max: Some(200.0),
            }),
        }),
        tools: None,
        memory: Some(MemoryLimits {
            working_context: Some(EnumConstraint {
                values: vec!["fixed".into(), "dynamic".into()],
            }),
            semantic_store_access: None,
        }),
        snapshot: Some(SnapshotLimits {
            enabled: Some(BoolConstraint { value: None }), // not locked
            auto_snapshot_interval_sec: Some(RangeConstraint {
                min: Some(0.0),
                max: Some(3_600.0),
            }),
        }),
        environment: Some(EnvironmentLimits {
            temperature: Some(RangeConstraint {
                min: Some(0.0),
                max: Some(2.0),
            }),
            timeout_sec: Some(RangeConstraint {
                min: Some(30.0),
                max: Some(600.0),
            }),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::defaults::{AgentDefaults, EntrypointDefaults};

    fn spec_example_yaml() -> &'static str {
        r#"
apiVersion: avix/v1
kind: Limits
metadata:
  target: agent-manifest
  layer: system
  owner: null
  updatedAt: "2026-03-15T07:38:00-05:00"
  updatedBy: kernel
  reason: boot
limits:
  entrypoint:
    modelPreference:
      type: enum
      values:
        - claude-sonnet-4
        - claude-haiku-4
    maxToolChain:
      type: range
      min: 1
      max: 10
  environment:
    temperature:
      type: range
      min: 0.0
      max: 1.0
    timeoutSec:
      type: range
      min: 30
      max: 600
"#
    }

    #[test]
    fn limits_file_parses_spec_example() {
        let file = LimitsFile::from_str(spec_example_yaml()).unwrap();
        assert_eq!(file.metadata.target, "agent-manifest");
        assert_eq!(file.metadata.layer, LimitsLayer::System);
        let limits = file.as_agent_limits().unwrap();
        let ep = limits.entrypoint.unwrap();
        assert_eq!(ep.max_tool_chain.as_ref().unwrap().max, Some(10.0));
    }

    #[test]
    fn agent_limits_intersect_tightest_range() {
        let researchers = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                max_tool_chain: Some(RangeConstraint {
                    min: None,
                    max: Some(10.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let writers = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                max_tool_chain: Some(RangeConstraint {
                    min: None,
                    max: Some(5.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let effective = researchers.intersect(&writers);
        assert_eq!(
            effective.entrypoint.unwrap().max_tool_chain.unwrap().max,
            Some(5.0)
        );
    }

    #[test]
    fn agent_limits_check_defaults_violation() {
        let limits = AgentLimits {
            entrypoint: Some(EntrypointLimits {
                max_tool_chain: Some(RangeConstraint {
                    min: None,
                    max: Some(5.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let defaults = AgentDefaults {
            entrypoint: Some(EntrypointDefaults {
                max_tool_chain: Some(20),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = limits.check_defaults(&defaults);
        assert!(result.is_err());
        let violations = result.unwrap_err();
        assert_eq!(violations[0].field, "entrypoint.maxToolChain");
    }

    #[test]
    fn agent_limits_check_defaults_passes_within_limits() {
        let limits = AgentLimits {
            environment: Some(EnvironmentLimits {
                timeout_sec: Some(RangeConstraint {
                    min: Some(30.0),
                    max: Some(600.0),
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let defaults = AgentDefaults {
            environment: Some(crate::params::defaults::EnvironmentDefaults {
                timeout_sec: Some(300),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(limits.check_defaults(&defaults).is_ok());
    }

    #[test]
    fn limits_file_round_trips() {
        let l = system_agent_limits();
        let yaml = LimitsFile::from_agent_limits(LimitsLayer::System, None, &l).unwrap();
        let parsed = LimitsFile::from_str(&yaml).unwrap();
        let l2 = parsed.as_agent_limits().unwrap();
        assert_eq!(l, l2);
    }
}
