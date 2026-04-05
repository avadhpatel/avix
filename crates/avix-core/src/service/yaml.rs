use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent_manifest::schema::{ManifestMetadata, PackagingMetadata};
use crate::error::AvixError;

// ── ServiceManifest (on-disk format) ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: ManifestMetadata,
    #[serde(default)]
    pub packaging: PackagingMetadata,
    pub spec: ServiceSpec,
}

impl ServiceManifest {
    pub fn load(path: &Path) -> Result<Self, AvixError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            AvixError::ConfigParse(format!("cannot read {}: {e}", path.display()))
        })?;
        serde_yaml::from_str(&content)
            .map_err(|e| AvixError::ConfigParse(format!("manifest.yaml parse error: {e}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    pub binary: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default = "default_restart_delay")]
    pub restart_delay: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_queue_timeout")]
    pub queue_timeout: String,
    #[serde(default)]
    pub run_as: RunAs,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    pub tools: ToolsSection,
    #[serde(default)]
    pub jobs: JobsSection,
}

// ── ServiceUnit (internal runtime struct) ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceUnit {
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
    /// Load a service manifest from the given path (must be a `manifest.yaml`).
    pub fn load(path: &Path) -> Result<Self, AvixError> {
        let m = ServiceManifest::load(path)?;
        Ok(Self::from_manifest(&m))
    }

    /// Convert from the on-disk `ServiceManifest` to the internal runtime struct.
    pub fn from_manifest(m: &ServiceManifest) -> Self {
        Self {
            name: m.metadata.name.clone(),
            version: m.metadata.version.clone(),
            source: ServiceSource::System,
            signature: m.packaging.signature.clone(),
            unit: UnitSection {
                description: m.metadata.description.clone(),
                author: m.metadata.author.clone(),
                requires: m.spec.requires.clone(),
                after: m.spec.after.clone(),
            },
            service: ServiceSection {
                binary: m.spec.binary.clone(),
                language: m.spec.language.clone(),
                restart: m.spec.restart.clone(),
                restart_delay: m.spec.restart_delay.clone(),
                max_concurrent: m.spec.max_concurrent,
                queue_max: m.spec.queue_max,
                queue_timeout: m.spec.queue_timeout.clone(),
                run_as: m.spec.run_as.clone(),
            },
            capabilities: m.spec.capabilities.clone(),
            tools: m.spec.tools.clone(),
            jobs: m.spec.jobs.clone(),
        }
    }

    pub fn load_for_service(root: &Path, name: &str) -> Result<Self, AvixError> {
        let services_dir = root.join("data").join("services");
        let mut found_path = None;

        if let Ok(entries) = std::fs::read_dir(&services_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    if let Ok(dir_name) = entry.file_name().into_string() {
                        if dir_name.starts_with(&format!("{}@", name)) {
                            found_path = Some(entry.path().join("manifest.yaml"));
                            break;
                        }
                    }
                }
            }
        }

        let path = found_path
            .ok_or_else(|| AvixError::ConfigParse(format!("service not found: {}", name)))?;
        Self::load(&path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceSource {
    System,
    Community,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UnitSection {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceSection {
    pub binary: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default = "default_restart_delay")]
    pub restart_delay: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_queue_timeout")]
    pub queue_timeout: String,
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
    #[serde(other)]
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolsSection {
    pub namespace: String,
    #[serde(default)]
    pub provides: Vec<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_manifest(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let path = dir.path().join("manifest.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    const MINIMAL_MANIFEST: &str = r#"
apiVersion: avix/v1
kind: Service
metadata:
  name: github-svc
  version: 1.0.0
  description: GitHub integration
spec:
  binary: /services/github-svc/bin/github-svc
  tools:
    namespace: /tools/github/
    provides:
      - list-prs
      - create-issue
"#;

    #[test]
    fn minimal_unit_parses() {
        let dir = TempDir::new().unwrap();
        let path = write_manifest(&dir, MINIMAL_MANIFEST);
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
        let path = write_manifest(
            &dir,
            r#"
apiVersion: avix/v1
kind: Service
metadata:
  name: min-svc
  version: 0.1.0
spec:
  binary: /bin/min-svc
  tools:
    namespace: /tools/min/
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
        let path = write_manifest(
            &dir,
            r#"
apiVersion: avix/v1
kind: Service
metadata:
  name: multi-svc
  version: 1.0.0
spec:
  binary: /bin/multi-svc
  capabilities:
    callerScoped: true
    required:
      - fs:read
    hostAccess:
      - network
  tools:
    namespace: /tools/multi/
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
            let w: W = serde_yaml::from_str(&format!("restart: {s}")).unwrap();
            assert_eq!(w.restart, expected);
        }
    }

    #[test]
    fn missing_binary_errors() {
        let dir = TempDir::new().unwrap();
        let path = write_manifest(
            &dir,
            r#"
apiVersion: avix/v1
kind: Service
metadata:
  name: bad
  version: 1.0.0
spec:
  tools:
    namespace: /tools/bad/
"#,
        );
        assert!(ServiceUnit::load(&path).is_err());
    }

    #[test]
    fn load_for_service_constructs_correct_path() {
        let dir = TempDir::new().unwrap();
        let svc_dir = dir.path().join("data").join("services").join("my-svc@1.0.0");
        std::fs::create_dir_all(&svc_dir).unwrap();
        std::fs::write(svc_dir.join("manifest.yaml"), MINIMAL_MANIFEST.replace("github-svc", "my-svc")).unwrap();
        let unit = ServiceUnit::load_for_service(dir.path(), "my-svc").unwrap();
        assert_eq!(unit.name, "my-svc");
    }

    #[test]
    fn from_manifest_maps_fields_correctly() {
        let m = ServiceManifest {
            api_version: "avix/v1".into(),
            kind: "Service".into(),
            metadata: ManifestMetadata {
                name: "test-svc".into(),
                version: "2.0.0".into(),
                description: "Test service".into(),
                author: "test-team".into(),
                license: None,
                tags: vec![],
                created_at: None,
            },
            packaging: PackagingMetadata {
                source: Some("system".into()),
                signature: Some("sha256:".into()),
            },
            spec: ServiceSpec {
                binary: "/bin/test-svc".into(),
                language: "rust".into(),
                restart: RestartPolicy::Always,
                restart_delay: "10s".into(),
                max_concurrent: 5,
                queue_max: 50,
                queue_timeout: "2s".into(),
                run_as: RunAs::Service,
                requires: vec!["memfs.svc".into()],
                after: vec!["router.svc".into()],
                capabilities: CapabilitiesSection {
                    caller_scoped: true,
                    required: vec!["fs:read".into()],
                    host_access: vec![HostAccess::Network],
                    scope: None,
                },
                tools: ToolsSection {
                    namespace: "/tools/test/".into(),
                    provides: vec!["list".into()],
                },
                jobs: JobsSection {
                    max_active: 2,
                    job_timeout: "600s".into(),
                    persist: true,
                },
            },
        };

        let unit = ServiceUnit::from_manifest(&m);
        assert_eq!(unit.name, "test-svc");
        assert_eq!(unit.version, "2.0.0");
        assert_eq!(unit.unit.description, "Test service");
        assert_eq!(unit.unit.after, vec!["router.svc"]);
        assert_eq!(unit.service.binary, "/bin/test-svc");
        assert_eq!(unit.service.restart, RestartPolicy::Always);
        assert_eq!(unit.service.max_concurrent, 5);
        assert!(unit.capabilities.caller_scoped);
        assert_eq!(unit.tools.namespace, "/tools/test/");
        assert_eq!(unit.jobs.max_active, 2);
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
