# Svc Gap C — Tool Descriptor Scanner (`*.tool.yaml`)

> **Status:** Pending
> **Priority:** High
> **Depends on:** Svc gap A (`ServiceUnit` types), gap B (service spawner)
> **Blocks:** Svc gaps D, F
> **Affects:** `crates/avix-core/src/tool_registry/` (new files),
>   `crates/avix-core/src/service/lifecycle.rs`

---

## Problem

`ToolEntry` holds a raw `serde_json::Value` descriptor. There is no typed `ToolDescriptor`
struct, no scanner that reads `services/<name>/tools/*.tool.yaml` files from disk, and
no path in the bootstrap that populates the `ToolRegistry` from installed services.

The architecture doc (`07-services.md`) defines the tool descriptor YAML format fully.
The spec (`service-authoring.md §9`) defines where these files live in the package.

---

## Scope

Define a typed `ToolDescriptor`, a scanner that loads `*.tool.yaml` from a service
directory, and wire the scanner into `ServiceManager::handle_ipc_register` so tools are
registered in the `ToolRegistry` immediately after a service registers. No VFS write —
tools live in the in-memory registry only.

---

## What Needs to Be Built

### 1. `tool_registry/descriptor.rs` — `ToolDescriptor`

```rust
use serde::{Deserialize, Serialize};

/// Typed tool descriptor, parsed from `<name>.tool.yaml`.
/// Matches the format defined in docs/architecture/07-services.md § Tool Descriptor Format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,           // VFS path: /tools/<ns>/<name>
    #[serde(default)]
    pub owner: Option<String>,          // service name
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: ToolDescriptorStatus,
    #[serde(default)]
    pub ipc: Option<IpcBinding>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub job: bool,
    #[serde(default)]
    pub job_timeout: Option<String>,
    #[serde(default)]
    pub capabilities_required: Vec<String>,
    #[serde(default)]
    pub input: serde_json::Value,       // schema kept as raw JSON for flexibility
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub visibility: ToolVisibilitySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ToolDescriptorStatus {
    #[serde(default = "default_state")]
    pub state: String,   // "available" | "degraded" | "unavailable"
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_state() -> String { "available".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcBinding {
    #[serde(default = "default_transport")]
    pub transport: String,      // always "local-ipc"
    pub endpoint: String,       // service name, e.g. "memfs"
    pub method: String,         // e.g. "fs.read"
}

fn default_transport() -> String { "local-ipc".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolVisibilitySpec {
    #[default]
    All,
    #[serde(rename = "user")]
    User(String),
    #[serde(rename = "crew")]
    Crew(String),
}
```

### 2. `tool_registry/scanner.rs` — `ToolScanner`

```rust
use std::path::Path;
use crate::error::AvixError;
use super::descriptor::ToolDescriptor;

pub struct ToolScanner;

impl ToolScanner {
    /// Scan `service_dir/tools/` for `*.tool.yaml` files and return parsed descriptors.
    /// `service_dir` is `AVIX_ROOT/services/<name>/`.
    /// Missing `tools/` directory → empty vec (not an error).
    pub fn scan(service_dir: &Path) -> Result<Vec<ToolDescriptor>, AvixError> {
        let tools_dir = service_dir.join("tools");
        if !tools_dir.exists() { return Ok(vec![]); }
        let mut descriptors = Vec::new();
        for entry in std::fs::read_dir(&tools_dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") { continue; }
            let name = path.file_name().unwrap().to_string_lossy();
            if !name.ends_with(".tool.yaml") { continue; }
            let content = std::fs::read_to_string(&path)
                .map_err(|e| AvixError::ConfigParse(format!("{}: {e}", path.display())))?;
            let desc: ToolDescriptor = serde_yaml::from_str(&content)
                .map_err(|e| AvixError::ConfigParse(format!("{}: {e}", path.display())))?;
            descriptors.push(desc);
        }
        Ok(descriptors)
    }

    /// Scan and convert descriptors to `ToolEntry` records ready for the registry.
    pub fn scan_as_entries(
        service_name: &str,
        service_dir: &Path,
    ) -> Result<Vec<crate::tool_registry::ToolEntry>, AvixError> {
        use crate::tool_registry::ToolEntry;
        use crate::types::tool::{ToolName, ToolState, ToolVisibility};
        use super::descriptor::ToolVisibilitySpec;

        Self::scan(service_dir)?
            .into_iter()
            .filter_map(|desc| {
                ToolName::parse(&desc.name)
                    .ok()
                    .map(|name| ToolEntry {
                        name,
                        owner: service_name.to_string(),
                        state: match desc.status.state.as_str() {
                            "available" => ToolState::Available,
                            "degraded"  => ToolState::Degraded,
                            _           => ToolState::Unavailable,
                        },
                        visibility: match &desc.visibility {
                            ToolVisibilitySpec::All        => ToolVisibility::All,
                            ToolVisibilitySpec::User(u)    => ToolVisibility::User(u.clone()),
                            ToolVisibilitySpec::Crew(c)    => ToolVisibility::Crew(c.clone()),
                        },
                        descriptor: serde_json::to_value(&desc).unwrap_or_default(),
                    })
            })
            .collect::<Vec<_>>()
            .pipe(Ok)
    }
}
```

> Note: `.pipe(Ok)` pattern — add a `pipe` helper or just use a let binding.

### 3. Wire into `ServiceManager::handle_ipc_register`

After successful registration, scan the service's tools directory and add them to the
registry:

```rust
pub async fn handle_ipc_register(
    &self,
    req: IpcRegisterRequest,
    service_root: &Path,   // new parameter: AVIX_ROOT
) -> Result<IpcRegisterResult, AvixError> {
    // ... existing validation and endpoint recording ...

    // Scan and register tools
    let svc_dir = service_root.join("services").join(&svc_name);
    let entries = ToolScanner::scan_as_entries(&svc_name, &svc_dir)?;
    if let Some(reg) = &self.tool_registry {
        reg.add(&svc_name, entries).await?;
    }

    Ok(IpcRegisterResult { registered: true, pid })
}
```

### 4. `mod.rs` exports

```rust
// tool_registry/mod.rs — add:
pub mod descriptor;
pub mod scanner;
pub use descriptor::ToolDescriptor;
pub use scanner::ToolScanner;
```

---

## Tests

```rust
// tool_registry/scanner.rs #[cfg(test)]
use tempfile::TempDir;

fn write_tool(dir: &TempDir, filename: &str, content: &str) {
    let tools_dir = dir.path().join("tools");
    std::fs::create_dir_all(&tools_dir).unwrap();
    std::fs::write(tools_dir.join(filename), content).unwrap();
}

#[test]
fn parses_minimal_tool_descriptor() {
    let dir = TempDir::new().unwrap();
    write_tool(&dir, "fs-read.tool.yaml", r#"
name: fs/read
description: Read file contents
capabilities_required: [fs:read]
input:
  path: { type: string, required: true }
output:
  content: { type: string }
"#);
    let descs = ToolScanner::scan(dir.path()).unwrap();
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].name, "fs/read");
    assert_eq!(descs[0].description, "Read file contents");
}

#[test]
fn skips_non_tool_yaml_files() {
    let dir = TempDir::new().unwrap();
    write_tool(&dir, "README.md", "# readme");
    write_tool(&dir, "config.yaml", "key: val");
    write_tool(&dir, "fs-read.tool.yaml", "name: fs/read\ndescription: x\n");
    let descs = ToolScanner::scan(dir.path()).unwrap();
    assert_eq!(descs.len(), 1);
}

#[test]
fn empty_vec_when_no_tools_dir() {
    let dir = TempDir::new().unwrap();
    let descs = ToolScanner::scan(dir.path()).unwrap();
    assert!(descs.is_empty());
}

#[test]
fn scan_multiple_tools() {
    let dir = TempDir::new().unwrap();
    for n in ["github-list-prs", "github-create-issue"] {
        write_tool(&dir, &format!("{n}.tool.yaml"),
            &format!("name: github/{}\ndescription: tool\n",
                n.replace("github-", "")));
    }
    let descs = ToolScanner::scan(dir.path()).unwrap();
    assert_eq!(descs.len(), 2);
}

#[test]
fn scan_as_entries_produces_tool_entries() {
    let dir = TempDir::new().unwrap();
    write_tool(&dir, "list-prs.tool.yaml",
        "name: github/list-prs\ndescription: List PRs\n");
    let entries = ToolScanner::scan_as_entries("github-svc", dir.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].owner, "github-svc");
}

#[test]
fn tool_descriptor_streaming_defaults_false() {
    let desc: ToolDescriptor = serde_yaml::from_str("name: x/y\ndescription: d\n").unwrap();
    assert!(!desc.streaming);
    assert!(!desc.job);
}

#[test]
fn tool_descriptor_job_flag() {
    let desc: ToolDescriptor = serde_yaml::from_str(
        "name: video/transcode\ndescription: Encode\njob: true\njob_timeout: 3600s\n"
    ).unwrap();
    assert!(desc.job);
    assert_eq!(desc.job_timeout.as_deref(), Some("3600s"));
}

#[test]
fn invalid_yaml_returns_error() {
    let dir = TempDir::new().unwrap();
    write_tool(&dir, "bad.tool.yaml", "name: [invalid yaml{{");
    assert!(ToolScanner::scan(dir.path()).is_err());
}
```

---

## Success Criteria

- [ ] `ToolScanner::scan` parses `*.tool.yaml` files; skips non-tool files
- [ ] Missing `tools/` directory returns empty vec, not error
- [ ] `scan_as_entries` converts descriptors to `ToolEntry` with correct owner/state/visibility
- [ ] `job: true` and `streaming: true` fields parse correctly
- [ ] Invalid YAML returns a descriptive error
- [ ] `handle_ipc_register` adds scanned tools to registry after registration
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
