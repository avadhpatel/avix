use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AvixError;

/// Top-level `service.unit` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceUnit {
    // ── Identity ──────────────────────────────────────────────────────────
    pub name: String,
    pub version: String,
    #[serde(default = "default_source")]
    pub source: ServiceSource,
    #[serde(default)]
    pub signature: Option<String>,

    pub unit: UnitSection,
    pub service: ServiceSection,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    pub tools: ToolsSection,
    #[serde(default)]
    pub jobs: JobsSection,
}

fn default_source() -> ServiceSource {
    ServiceSource::User
}

impl ServiceUnit {
    /// Load and parse a `service.unit` file from `path`.
    pub fn load(path: &Path) -> Result<Self, AvixError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AvixError::ConfigParse(format!("cannot read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| AvixError::ConfigParse(format!("service.unit parse error: {e}")))
    }

    /// Load from `AVIX_ROOT/services/<name>/service.unit`.
    pub fn load_for_service(root: &Path, name: &str) -> Result<Self, AvixError> {
        Self::load(&root.join("services").join(name).join("service.unit"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceSource {
    System,
    Community,
    User,
}

// ── [unit] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UnitSection {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
}

// ── [service] ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceSection {
    pub binary: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default = "default_restart_delay")]
    pub restart_delay: String, // e.g. "5s" — parsed to Duration by callers
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_queue_timeout")]
    pub queue_timeout: String, // e.g. "5s"
    #[serde(default)]
    pub run_as: RunAs,
}

fn default_language() -> String {
    "any".into()
}
fn default_restart_delay() -> String {
    "5s".into()
}
fn default_max_concurrent() -> u32 {
    20
}
fn default_queue_max() -> u32 {
    100
}
fn default_queue_timeout() -> String {
    "5s".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    #[default]
    OnFailure,
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RunAs {
    #[default]
    Service,
    // "user:<username>" — parsed as a special case
    #[serde(other)]
    User,
}

// ── [capabilities] ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CapabilitiesSection {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub host_access: Vec<HostAccess>,
    #[serde(default)]
    pub caller_scoped: bool,
}

/// Host access grant. Serialises/deserialises as a plain string:
/// - `"network"`
/// - `"filesystem:<path>"`
/// - `"socket:<path>"`
/// - `"env:<VAR>"`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAccess {
    Network,
    Filesystem(String),
    Socket(String),
    Env(String),
}

impl Serialize for HostAccess {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let val = match self {
            HostAccess::Network => "network".to_string(),
            HostAccess::Filesystem(p) => format!("filesystem:{p}"),
            HostAccess::Socket(p) => format!("socket:{p}"),
            HostAccess::Env(v) => format!("env:{v}"),
        };
        s.serialize_str(&val)
    }
}

impl<'de> Deserialize<'de> for HostAccess {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "network" {
            return Ok(HostAccess::Network);
        }
        if let Some(path) = s.strip_prefix("filesystem:") {
            return Ok(HostAccess::Filesystem(path.to_string()));
        }
        if let Some(path) = s.strip_prefix("socket:") {
            return Ok(HostAccess::Socket(path.to_string()));
        }
        if let Some(var) = s.strip_prefix("env:") {
            return Ok(HostAccess::Env(var.to_string()));
        }
        Err(serde::de::Error::custom(format!(
            "unknown host_access value: {s}"
        )))
    }
}

// ── [tools] ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolsSection {
    pub namespace: String,
    #[serde(default)]
    pub provides: Vec<String>,
}

// ── [jobs] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct JobsSection {
    #[serde(default = "default_max_active")]
    pub max_active: u32,
    #[serde(default = "default_job_timeout")]
    pub job_timeout: String,
    #[serde(default)]
    pub persist: bool,
}

fn default_max_active() -> u32 {
    3
}
fn default_job_timeout() -> String {
    "3600s".into()
}

// ── Duration helper ───────────────────────────────────────────────────────────

/// Parse a duration string like `"5s"`, `"60s"`, `"1m"` into [`std::time::Duration`].
pub fn parse_duration(s: &str) -> Result<std::time::Duration, AvixError> {
    if let Some(n) = s.strip_suffix('s') {
        let secs: u64 = n
            .parse()
            .map_err(|_| AvixError::ConfigParse(format!("invalid duration: {s}")))?;
        return Ok(std::time::Duration::from_secs(secs));
    }
    if let Some(n) = s.strip_suffix('m') {
        let mins: u64 = n
            .parse()
            .map_err(|_| AvixError::ConfigParse(format!("invalid duration: {s}")))?;
        return Ok(std::time::Duration::from_secs(mins * 60));
    }
    Err(AvixError::ConfigParse(format!(
        "unsupported duration format: {s}"
    )))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_unit(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("service.unit");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn minimal_unit_parses() {
        let dir = TempDir::new().unwrap();
        let path = write_unit(
            &dir,
            r#"
name    = "github-svc"
version = "1.0.0"

[unit]
description = "GitHub integration"

[service]
binary = "/services/github-svc/bin/github-svc"

[tools]
namespace = "/tools/github/"
provides  = ["list-prs", "create-issue"]
"#,
        );
        let unit = ServiceUnit::load(&path).unwrap();
        assert_eq!(unit.name, "github-svc");
        assert_eq!(unit.version, "1.0.0");
        assert_eq!(unit.service.binary, "/services/github-svc/bin/github-svc");
        assert_eq!(unit.tools.namespace, "/tools/github/");
        assert_eq!(unit.tools.provides.len(), 2);
    }

    #[test]
    fn defaults_are_applied() {
        let dir = TempDir::new().unwrap();
        let path = write_unit(
            &dir,
            r#"
name    = "min-svc"
version = "0.1.0"
[unit]
[service]
binary = "/bin/min-svc"
[tools]
namespace = "/tools/min/"
"#,
        );
        let unit = ServiceUnit::load(&path).unwrap();
        assert_eq!(unit.service.max_concurrent, 20);
        assert_eq!(unit.service.queue_max, 100);
        assert_eq!(unit.service.restart, RestartPolicy::OnFailure);
        assert!(!unit.capabilities.caller_scoped);
        assert!(unit.capabilities.host_access.is_empty());
    }

    #[test]
    fn caller_scoped_and_host_access_parse() {
        let dir = TempDir::new().unwrap();
        let path = write_unit(
            &dir,
            r#"
name    = "multi-svc"
version = "1.0.0"
[unit]
[service]
binary = "/bin/multi-svc"
[capabilities]
caller_scoped = true
required = ["fs:read"]
host_access = ["network"]
[tools]
namespace = "/tools/multi/"
"#,
        );
        let unit = ServiceUnit::load(&path).unwrap();
        assert!(unit.capabilities.caller_scoped);
        assert_eq!(unit.capabilities.required, vec!["fs:read"]);
        assert!(matches!(
            unit.capabilities.host_access[0],
            HostAccess::Network
        ));
    }

    #[test]
    fn restart_policy_variants() {
        #[derive(serde::Deserialize)]
        struct W {
            restart: RestartPolicy,
        }
        for (s, expected) in [
            ("on-failure", RestartPolicy::OnFailure),
            ("always", RestartPolicy::Always),
            ("never", RestartPolicy::Never),
        ] {
            let w: W = toml::from_str(&format!("restart = \"{s}\"")).unwrap();
            assert_eq!(w.restart, expected);
        }
    }

    #[test]
    fn missing_binary_errors() {
        let dir = TempDir::new().unwrap();
        // Intentionally malformed TOML — binary field is missing
        let path = write_unit(
            &dir,
            r#"
name = "bad"
version = "1.0.0"
[unit]
[service]
[tools]
namespace = "/tools/bad/"
"#,
        );
        assert!(ServiceUnit::load(&path).is_err());
    }

    #[test]
    fn load_for_service_constructs_correct_path() {
        let dir = TempDir::new().unwrap();
        let svc_dir = dir.path().join("services").join("my-svc");
        std::fs::create_dir_all(&svc_dir).unwrap();
        let content = r#"
name = "my-svc"
version = "1.0.0"
[unit]
[service]
binary = "/bin/my-svc"
[tools]
namespace = "/tools/my/"
"#;
        std::fs::write(svc_dir.join("service.unit"), content).unwrap();
        let unit = ServiceUnit::load_for_service(dir.path(), "my-svc").unwrap();
        assert_eq!(unit.name, "my-svc");
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(
            parse_duration("5s").unwrap(),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            parse_duration("60s").unwrap(),
            std::time::Duration::from_secs(60)
        );
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(
            parse_duration("2m").unwrap(),
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn parse_duration_invalid() {
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn host_access_roundtrip() {
        let cases = vec![
            HostAccess::Network,
            HostAccess::Filesystem("/tmp/data".to_string()),
            HostAccess::Socket("/var/run/foo.sock".to_string()),
            HostAccess::Env("MY_VAR".to_string()),
        ];
        for ha in cases {
            let serialized = serde_json::to_string(&ha).unwrap();
            let deserialized: HostAccess = serde_json::from_str(&serialized).unwrap();
            assert_eq!(ha, deserialized);
        }
    }
}
