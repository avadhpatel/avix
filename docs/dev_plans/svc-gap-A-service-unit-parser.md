# Svc Gap A — `service.unit` Parser and Types

> **Status:** Pending
> **Priority:** Critical — all other service gaps depend on this
> **Depends on:** nothing
> **Blocks:** Svc gaps B, C, D, E, F, G, H
> **Affects:** `crates/avix-core/src/service/` (new files)

---

## Problem

There is no `service.unit` file parser. The `ServiceManager` has no knowledge of the
unit file format — it only accepts name+binary strings directly. Every downstream gap
(spawning, installing, restart policy, caller scoping) requires a typed `ServiceUnit`
struct parsed from disk.

Both the spec (`docs/spec/service-authoring.md`) and the architecture doc
(`docs/architecture/07-services.md`) define the format. The spec uses TOML syntax;
the arch doc uses YAML. **Implement as TOML** (the spec is authoritative for the
authoring surface; YAML is the architecture doc's internal notation).

---

## Scope

Define all `ServiceUnit` types and a parser that loads a `service.unit` file from
`AVIX_ROOT/services/<name>/service.unit`. No process spawning. No VFS writes. Types
and parser only.

---

## What Needs to Be Built

### 1. New file: `crates/avix-core/src/service/unit.rs`

```rust
use serde::{Deserialize, Serialize};

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

fn default_source() -> ServiceSource { ServiceSource::User }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceSource {
    System,
    Community,
    User,
}

// ── [unit] ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub restart_delay: String,   // e.g. "5s" — parsed to Duration by callers
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_queue_timeout")]
    pub queue_timeout: String,   // e.g. "5s"
    #[serde(default)]
    pub run_as: RunAs,
}

fn default_language() -> String { "any".into() }
fn default_restart_delay() -> String { "5s".into() }
fn default_max_concurrent() -> u32 { 20 }
fn default_queue_max() -> u32 { 100 }
fn default_queue_timeout() -> String { "5s".into() }

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HostAccess {
    Network,
    #[serde(rename = "filesystem")]
    Filesystem(String),   // filesystem:<path>
    #[serde(rename = "socket")]
    Socket(String),       // socket:<path>
    #[serde(rename = "env")]
    Env(String),          // env:<VAR>
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

fn default_max_active() -> u32 { 3 }
fn default_job_timeout() -> String { "3600s".into() }
```

### 2. `ServiceUnit::load(path: &Path) -> Result<ServiceUnit, AvixError>`

```rust
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
```

### 3. Duration helper

```rust
/// Parse a duration string like "5s", "60s", "1m" into `std::time::Duration`.
pub fn parse_duration(s: &str) -> Result<std::time::Duration, AvixError> {
    if let Some(n) = s.strip_suffix('s') {
        let secs: u64 = n.parse().map_err(|_| AvixError::ConfigParse(format!("invalid duration: {s}")))?;
        return Ok(std::time::Duration::from_secs(secs));
    }
    if let Some(n) = s.strip_suffix('m') {
        let mins: u64 = n.parse().map_err(|_| AvixError::ConfigParse(format!("invalid duration: {s}")))?;
        return Ok(std::time::Duration::from_secs(mins * 60));
    }
    Err(AvixError::ConfigParse(format!("unsupported duration format: {s}")))
}
```

### 4. Wire `unit.rs` into `service/mod.rs`

```rust
pub mod unit;
pub use unit::{ServiceUnit, RestartPolicy, ServiceSource, HostAccess, parse_duration};
```

### 5. Add `toml` to `avix-core/Cargo.toml`

```toml
toml = "0.8"
```

---

## `.install.json` Receipt type

Also add `InstallReceipt` (used in gap D):

```rust
// service/install_receipt.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReceipt {
    pub name: String,
    pub version: String,
    pub source_url: Option<String>,
    pub checksum: Option<String>,          // "sha256:abc123..."
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub service_unit_path: String,
    pub binary_path: String,
}
```

---

## Tests (in `service/unit.rs` under `#[cfg(test)]`)

```rust
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
        let path = write_unit(&dir, r#"
name    = "github-svc"
version = "1.0.0"

[unit]
description = "GitHub integration"

[service]
binary = "/services/github-svc/bin/github-svc"

[tools]
namespace = "/tools/github/"
provides  = ["list-prs", "create-issue"]
"#);
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
        let path = write_unit(&dir, r#"
name    = "min-svc"
version = "0.1.0"
[unit]
[service]
binary = "/bin/min-svc"
[tools]
namespace = "/tools/min/"
"#);
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
        let path = write_unit(&dir, r#"
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
"#);
        let unit = ServiceUnit::load(&path).unwrap();
        assert!(unit.capabilities.caller_scoped);
        assert_eq!(unit.capabilities.required, vec!["fs:read"]);
        assert!(matches!(unit.capabilities.host_access[0], HostAccess::Network));
    }

    #[test]
    fn restart_policy_variants() {
        for (s, expected) in [
            ("on-failure", RestartPolicy::OnFailure),
            ("always", RestartPolicy::Always),
            ("never", RestartPolicy::Never),
        ] {
            let policy: RestartPolicy = toml::from_str(&format!("restart = \"{s}\""))
                .unwrap();
            assert_eq!(policy, expected);
        }
    }

    #[test]
    fn missing_binary_errors() {
        let dir = TempDir::new().unwrap();
        let path = write_unit(&dir, r#"
name = "bad" version = "1.0.0"
[unit] [service]
[tools] namespace = "/tools/bad/"
"#);
        assert!(ServiceUnit::load(&path).is_err());
    }

    #[test]
    fn load_for_service_constructs_correct_path() {
        let dir = TempDir::new().unwrap();
        let svc_dir = dir.path().join("services").join("my-svc");
        std::fs::create_dir_all(&svc_dir).unwrap();
        let content = r#"
name = "my-svc" version = "1.0.0"
[unit] [service]
binary = "/bin/my-svc"
[tools] namespace = "/tools/my/"
"#;
        std::fs::write(svc_dir.join("service.unit"), content).unwrap();
        let unit = ServiceUnit::load_for_service(dir.path(), "my-svc").unwrap();
        assert_eq!(unit.name, "my-svc");
    }

    #[test]
    fn parse_duration_seconds() {
        assert_eq!(parse_duration("5s").unwrap(), std::time::Duration::from_secs(5));
        assert_eq!(parse_duration("60s").unwrap(), std::time::Duration::from_secs(60));
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("2m").unwrap(), std::time::Duration::from_secs(120));
    }

    #[test]
    fn parse_duration_invalid() {
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("abc").is_err());
    }
}
```

---

## Success Criteria

- [ ] `ServiceUnit::load` parses a minimal unit file
- [ ] All defaults match the spec values
- [ ] `RestartPolicy`, `ServiceSource`, `HostAccess`, `RunAs` all deserialise correctly
- [ ] `load_for_service` constructs the correct path
- [ ] `parse_duration` handles `s` and `m` suffixes; errors on unknown
- [ ] `InstallReceipt` serialises and deserialises
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
