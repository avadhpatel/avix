# pkg-gap-E — Package Authoring Tooling

> **Status:** Pending
> **Priority:** High — needed before any agent/service packages can be published
> **Depends on:** nothing (pure local tooling, no ATP required)
> **Blocks:** nothing (but pkg-gap-A install depends on well-formed packages existing)
> **Affects:**
> - `crates/avix-core/src/packaging/` (new module)
> - `crates/avix-cli/src/main.rs` (new `Package` subcommand under `ClientCmd`)
> - `agents/packs/` and `services/` (repo directory conventions, no code)

---

## Problem

There is no tooling to build or validate Avix packages. Developers must manually write bash
`tar` commands to create archives, have no way to check a package is well-formed before
publishing, and the repo has no defined directory layout for agent packs or service source trees.
The GitHub Actions workflow (pkg-gap-C) can't work without knowing where the source directories are.

---

## Scope

1. **Repo directory conventions** — define where agent packs and service source trees live.
2. **`PackageValidator`** — validate a directory against the required structure for its type.
3. **`PackageBuilder`** — assemble a `.tar.xz` archive + `checksums.sha256` from a source directory.
4. **`avix package build`** CLI command — wraps `PackageBuilder`, entirely offline/local.
5. **`avix package validate`** CLI command — wraps `PackageValidator`, exits non-zero on failure.
6. **`avix package new`** CLI command — scaffold a new agent pack or service directory.

No ATP. No kernel interaction. All commands work without a running server.

---

## Repo Directory Conventions

```
agents/
└── packs/
    └── <agent-name>/          # e.g. universal-tool-explorer/
        ├── manifest.yaml      # REQUIRED
        ├── system-prompt.md   # REQUIRED (or referenced in manifest)
        ├── examples/          # optional
        └── README.md          # optional

services/
└── <service-name>/            # e.g. workspace/
    ├── Cargo.toml             # REQUIRED for Rust services
    ├── src/
    ├── service.unit           # REQUIRED
    ├── tools/                 # optional: *.tool.yaml descriptors
    └── README.md              # optional
```

The GitHub Actions workflow in pkg-gap-C references these exact paths.
Service binaries are built by CI into `build/package/bin/` at packaging time — the `bin/`
directory is never committed to source control.

---

## What to Build

### 1. New module: `crates/avix-core/src/packaging/`

```
crates/avix-core/src/packaging/
├── mod.rs
├── validator.rs
├── builder.rs
└── scaffold.rs
```

Wire into `avix-core/src/lib.rs`:
```rust
pub mod packaging;
```

---

### 2. `PackageType` — `packaging/mod.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageType {
    Agent,
    Service,
}

impl PackageType {
    /// Detect package type from directory: agent if `manifest.yaml` present, service if `service.unit`.
    pub fn detect(dir: &std::path::Path) -> Result<Self, AvixError> {
        if dir.join("manifest.yaml").exists() {
            return Ok(Self::Agent);
        }
        if dir.join("service.unit").exists() {
            return Ok(Self::Service);
        }
        Err(AvixError::ConfigParse(
            "cannot detect package type: no manifest.yaml or service.unit found".into(),
        ))
    }
}
```

---

### 3. `PackageValidator` — `packaging/validator.rs`

```rust
use std::path::Path;
use crate::error::AvixError;
use super::PackageType;

#[derive(Debug)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

pub struct PackageValidator;

impl PackageValidator {
    /// Validate `dir` against its detected package type.
    /// Returns all errors found (not just the first).
    pub fn validate(dir: &Path) -> Result<PackageType, Vec<ValidationError>> {
        let pkg_type = PackageType::detect(dir).map_err(|e| {
            vec![ValidationError { path: dir.display().to_string(), message: e.to_string() }]
        })?;
        let mut errors = Vec::new();
        match pkg_type {
            PackageType::Agent => Self::validate_agent(dir, &mut errors),
            PackageType::Service => Self::validate_service(dir, &mut errors),
        }
        if errors.is_empty() { Ok(pkg_type) } else { Err(errors) }
    }

    fn validate_agent(dir: &Path, errors: &mut Vec<ValidationError>) {
        // manifest.yaml — required, must parse as AgentManifestFile
        let manifest_path = dir.join("manifest.yaml");
        match std::fs::read_to_string(&manifest_path) {
            Err(e) => errors.push(ValidationError {
                path: "manifest.yaml".into(),
                message: format!("cannot read: {e}"),
            }),
            Ok(content) => {
                if let Err(e) = serde_yaml::from_str::<crate::agent_manifest::AgentManifestFile>(&content) {
                    errors.push(ValidationError {
                        path: "manifest.yaml".into(),
                        message: format!("parse error: {e}"),
                    });
                } else {
                    // name and version must be non-empty
                    let m: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or_default();
                    if m["name"].as_str().unwrap_or("").is_empty() {
                        errors.push(ValidationError { path: "manifest.yaml".into(), message: "name is empty".into() });
                    }
                    if m["version"].as_str().unwrap_or("").is_empty() {
                        errors.push(ValidationError { path: "manifest.yaml".into(), message: "version is empty".into() });
                    }
                    // system_prompt_path referenced → file must exist
                    if let Some(prompt_path) = m["system_prompt_path"].as_str() {
                        if !dir.join(prompt_path).exists() {
                            errors.push(ValidationError {
                                path: prompt_path.into(),
                                message: "system_prompt_path references missing file".into(),
                            });
                        }
                    }
                }
            }
        }
        // system-prompt.md — required if system_prompt_path not set in manifest
        // (checked above; also accept top-level system-prompt.md as implicit default)
        if !dir.join("manifest.yaml").exists() {
            return; // already reported above
        }
        // Warn (not error) if no README.md
        // (not a validation error, skip)
    }

    fn validate_service(dir: &Path, errors: &mut Vec<ValidationError>) {
        // service.unit — required, must parse as ServiceUnit
        let unit_path = dir.join("service.unit");
        match std::fs::read_to_string(&unit_path) {
            Err(e) => errors.push(ValidationError {
                path: "service.unit".into(),
                message: format!("cannot read: {e}"),
            }),
            Ok(content) => {
                if let Err(e) = toml::from_str::<crate::service::ServiceUnit>(&content) {
                    errors.push(ValidationError {
                        path: "service.unit".into(),
                        message: format!("parse error: {e}"),
                    });
                }
            }
        }
        // bin/ directory — required (must contain at least one executable file)
        let bin_dir = dir.join("bin");
        if !bin_dir.exists() {
            errors.push(ValidationError {
                path: "bin/".into(),
                message: "bin/ directory is missing (build the binary first)".into(),
            });
        } else {
            let has_binary = std::fs::read_dir(&bin_dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
            if !has_binary {
                errors.push(ValidationError {
                    path: "bin/".into(),
                    message: "bin/ directory is empty".into(),
                });
            }
        }
    }
}
```

---

### 4. `PackageBuilder` — `packaging/builder.rs`

```rust
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use crate::error::AvixError;
use super::{PackageType, PackageValidator};

pub struct BuildRequest {
    /// Source directory (agent pack or service package dir).
    pub source_dir: PathBuf,
    /// Output directory for the archive. Defaults to parent of source_dir.
    pub output_dir: PathBuf,
    /// Version string to embed in filename (e.g. "v0.1.0").
    pub version: String,
    /// Skip validation before building.
    pub skip_validation: bool,
}

pub struct BuildResult {
    pub archive_path: PathBuf,
    pub checksum_entry: String,   // "sha256:<hex>  <filename>\n" line
    pub pkg_type: PackageType,
    pub name: String,
    pub version: String,
}

pub struct PackageBuilder;

impl PackageBuilder {
    pub fn build(req: BuildRequest) -> Result<BuildResult, AvixError> {
        // 1. Validate unless skipped.
        let pkg_type = if req.skip_validation {
            PackageType::detect(&req.source_dir)?
        } else {
            PackageValidator::validate(&req.source_dir).map_err(|errs| {
                let msg = errs.iter()
                    .map(|e| format!("  {}: {}", e.path, e.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                AvixError::ConfigParse(format!("validation failed:\n{msg}"))
            })?
        };

        // 2. Read name from manifest / service.unit.
        let name = Self::read_name(&req.source_dir, &pkg_type)?;

        // 3. Determine OS/arch suffix for service archives.
        let filename = match &pkg_type {
            PackageType::Agent => {
                format!("{}-{}.tar.xz", name, req.version)
            }
            PackageType::Service => {
                let os = std::env::consts::OS;
                let arch = std::env::consts::ARCH;
                format!("{}-{}-{}-{}.tar.xz", name, req.version, os, arch)
            }
        };

        let archive_path = req.output_dir.join(&filename);
        std::fs::create_dir_all(&req.output_dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        // 4. Build the .tar.xz archive.
        Self::create_xz_archive(&req.source_dir, &archive_path)?;

        // 5. Compute SHA-256 and write/append checksums.sha256.
        let bytes = std::fs::read(&archive_path)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let checksum_entry = format!("{}  {}\n", digest, filename);

        let checksums_path = req.output_dir.join("checksums.sha256");
        let mut existing = if checksums_path.exists() {
            std::fs::read_to_string(&checksums_path)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        } else {
            String::new()
        };
        // Replace existing entry for this filename if present, else append.
        if existing.contains(&filename) {
            existing = existing
                .lines()
                .filter(|l| !l.contains(&filename))
                .map(|l| format!("{l}\n"))
                .collect();
        }
        existing.push_str(&checksum_entry);
        std::fs::write(&checksums_path, &existing)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(BuildResult {
            archive_path,
            checksum_entry,
            pkg_type,
            name,
            version: req.version,
        })
    }

    fn create_xz_archive(source_dir: &Path, dest: &Path) -> Result<(), AvixError> {
        let file = std::fs::File::create(dest)
            .map_err(|e| AvixError::ConfigParse(format!("create archive: {e}")))?;
        let xz = xz2::write::XzEncoder::new(file, 6); // compression level 6
        let mut archive = tar::Builder::new(xz);
        archive.follow_symlinks(false);

        // Walk source_dir, add each file with a path relative to source_dir.
        // Skip: .git/, target/, *.lock files.
        Self::add_dir_to_archive(&mut archive, source_dir, source_dir)?;

        archive.finish()
            .map_err(|e| AvixError::ConfigParse(format!("finalize archive: {e}")))?;
        Ok(())
    }

    fn add_dir_to_archive(
        archive: &mut tar::Builder<impl std::io::Write>,
        base: &Path,
        dir: &Path,
    ) -> Result<(), AvixError> {
        for entry in std::fs::read_dir(dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap();

            // Skip .git/, target/, Cargo.lock.
            let name = rel.components().next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            if matches!(name.as_str(), ".git" | "target" | "Cargo.lock") {
                continue;
            }

            if path.is_dir() {
                Self::add_dir_to_archive(archive, base, &path)?;
            } else {
                archive.append_path_with_name(&path, rel)
                    .map_err(|e| AvixError::ConfigParse(format!("add {}: {e}", rel.display())))?;
            }
        }
        Ok(())
    }

    fn read_name(dir: &Path, pkg_type: &PackageType) -> Result<String, AvixError> {
        match pkg_type {
            PackageType::Agent => {
                let content = std::fs::read_to_string(dir.join("manifest.yaml"))
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let m: serde_yaml::Value = serde_yaml::from_str(&content)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                m["name"].as_str()
                    .map(|s| s.to_owned())
                    .ok_or_else(|| AvixError::ConfigParse("manifest.yaml missing name".into()))
            }
            PackageType::Service => {
                let content = std::fs::read_to_string(dir.join("service.unit"))
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let u: toml::Value = toml::from_str(&content)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                u["name"].as_str()
                    .map(|s| s.to_owned())
                    .ok_or_else(|| AvixError::ConfigParse("service.unit missing name".into()))
            }
        }
    }
}
```

---

### 5. `PackageScaffold` — `packaging/scaffold.rs`

Creates a skeleton directory for a new agent pack or service.

```rust
pub struct ScaffoldRequest {
    pub name: String,
    pub pkg_type: PackageType,
    pub version: String,
    pub output_dir: std::path::PathBuf,
}

pub struct PackageScaffold;

impl PackageScaffold {
    pub fn create(req: ScaffoldRequest) -> Result<std::path::PathBuf, AvixError> {
        let dir = req.output_dir.join(&req.name);
        if dir.exists() {
            return Err(AvixError::ConfigParse(format!("directory already exists: {}", dir.display())));
        }
        match req.pkg_type {
            PackageType::Agent => Self::scaffold_agent(&dir, &req.name, &req.version),
            PackageType::Service => Self::scaffold_service(&dir, &req.name, &req.version),
        }?;
        Ok(dir)
    }

    fn scaffold_agent(dir: &std::path::Path, name: &str, version: &str) -> Result<(), AvixError> {
        std::fs::create_dir_all(dir.join("examples"))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("manifest.yaml"), format!(
            "name: {name}\nversion: \"{version}\"\ndescription: \"\"\nsystem_prompt_path: system-prompt.md\n"
        )).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("system-prompt.md"),
            format!("# {name}\n\nYou are a helpful agent.\n")
        ).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("README.md"),
            format!("# {name}\n\nDescribe your agent here.\n")
        ).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    fn scaffold_service(dir: &std::path::Path, name: &str, version: &str) -> Result<(), AvixError> {
        std::fs::create_dir_all(dir.join("src"))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::create_dir_all(dir.join("tools"))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("service.unit"), format!(
r#"name    = "{name}"
version = "{version}"

[unit]
description = ""
after       = ["router.svc"]

[service]
binary  = "/services/{name}/bin/{name}"
language = "rust"
restart = "on-failure"

[capabilities]
caller_scoped = false

[tools]
namespace = "/tools/{name}/"
provides  = []
"#
        )).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("Cargo.toml"), format!(
r#"[package]
name    = "{name}"
version = "{version}"
edition = "2021"

[[bin]]
name = "{name}"
path = "src/main.rs"
"#
        )).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello from {name}\");\n}\n"
                .replace("{name}", name)
        ).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(dir.join("README.md"),
            format!("# {name}\n\nDescribe your service here.\n")
        ).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }
}
```

---

### 6. CLI — `avix package` subcommand

Add `Package` to `ClientCmd` in `crates/avix-cli/src/main.rs`:

```rust
/// Build, validate, and scaffold Avix packages (offline — no server required)
Package {
    #[command(subcommand)]
    sub: PackageCmd,
},
```

```rust
#[derive(Subcommand)]
enum PackageCmd {
    /// Validate a package directory without building
    Validate {
        /// Path to the agent pack or service directory
        path: PathBuf,
    },
    /// Build a .tar.xz archive from a package directory
    Build {
        /// Path to the agent pack or service directory
        path: PathBuf,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// Version string (e.g. v0.1.0)
        #[arg(long)]
        version: String,
        /// Skip pre-build validation
        #[arg(long)]
        skip_validation: bool,
    },
    /// Scaffold a new agent pack or service directory
    New {
        /// Package name
        name: String,
        /// Package type: agent or service
        #[arg(long = "type", value_parser = ["agent", "service"])]
        pkg_type: String,
        /// Initial version (default: 0.1.0)
        #[arg(long, default_value = "0.1.0")]
        version: String,
        /// Output directory (default: current directory)
        #[arg(long, short = 'o', default_value = ".")]
        output: PathBuf,
    },
}
```

Handler logic in main (no ATP client needed):

```rust
Cmd::Client { sub: ClientCmd::Package { sub } } => match sub {
    PackageCmd::Validate { path } => {
        match PackageValidator::validate(&path) {
            Ok(pkg_type) => {
                println!("✓ Valid {:?} package", pkg_type);
            }
            Err(errors) => {
                eprintln!("Validation failed ({} error(s)):", errors.len());
                for e in &errors {
                    eprintln!("  {}: {}", e.path, e.message);
                }
                std::process::exit(1);
            }
        }
    }
    PackageCmd::Build { path, output, version, skip_validation } => {
        let output_dir = output.unwrap_or_else(|| std::env::current_dir().unwrap());
        let req = BuildRequest { source_dir: path, output_dir, version, skip_validation };
        let result = PackageBuilder::build(req)
            .context("package build failed")?;
        println!("Built: {}", result.archive_path.display());
        println!("Checksum: {}", result.checksum_entry.trim());
    }
    PackageCmd::New { name, pkg_type, version, output } => {
        let pkg_type = if pkg_type == "agent" { PackageType::Agent } else { PackageType::Service };
        let dir = PackageScaffold::create(ScaffoldRequest { name: name.clone(), pkg_type, version, output_dir: output })
            .context("scaffold failed")?;
        println!("Created: {}", dir.display());
    }
},
```

---

## `checksums.sha256` Format

Each line in `checksums.sha256` follows the standard `sha256sum` output format:

```
<hex-digest>  <filename>
```

Example:
```
a3f8c2d1e4b7...  universal-tool-explorer-v0.1.0.tar.xz
9f1b2e3d4c5a...  workspace-v1.0.0-linux-x86_64.tar.xz
```

Two spaces between digest and filename (standard `sha256sum` format so `sha256sum -c checksums.sha256`
works out of the box). `PackageBuilder` maintains this file incrementally — building multiple
packages into the same output dir accumulates all entries.

---

## Tests

All tests in `#[cfg(test)]` blocks within each module.

### `packaging/mod.rs`
- `detect_agent_from_manifest_yaml()` — dir with `manifest.yaml` → `PackageType::Agent`
- `detect_service_from_service_unit()` — dir with `service.unit` → `PackageType::Service`
- `detect_unknown_errors()` — empty dir → `Err`

### `packaging/validator.rs`
- `valid_agent_pack_passes()` — minimal valid agent dir → `Ok`
- `agent_missing_manifest_errors()` — no `manifest.yaml` → error list contains manifest entry
- `agent_empty_name_errors()` — `manifest.yaml` with `name: ""` → error
- `agent_missing_prompt_file_errors()` — `system_prompt_path` references absent file → error
- `valid_service_passes()` — service.unit + bin/svc → `Ok`
- `service_missing_unit_errors()` — no `service.unit` → error
- `service_missing_bin_errors()` — no `bin/` → error
- `service_empty_bin_errors()` — empty `bin/` dir → error

### `packaging/builder.rs`
- `build_agent_creates_tar_xz()` — build a minimal agent dir, check archive exists, is non-empty, decompresses correctly
- `build_service_creates_platform_archive()` — filename contains OS + arch
- `build_writes_checksums_file()` — `checksums.sha256` created, contains correct hex + filename
- `build_accumulates_checksums()` — build two packages to same output dir, both entries present
- `build_validates_before_build()` — invalid package → `Err` before creating archive
- `build_skip_validation_bypasses_check()` — invalid package + `skip_validation: true` → proceeds

### `packaging/scaffold.rs`
- `scaffold_agent_creates_required_files()` — `manifest.yaml` + `system-prompt.md` present after scaffold
- `scaffold_service_creates_required_files()` — `service.unit` + `Cargo.toml` + `src/main.rs` present
- `scaffold_existing_dir_errors()` — directory already exists → `Err`
- `scaffold_agent_manifest_is_valid_yaml()` — parse generated `manifest.yaml` → no error
- `scaffold_service_unit_is_valid_toml()` — parse generated `service.unit` → no error

---

## Success Criteria

- [ ] `avix package validate ./agents/packs/universal-tool-explorer` exits 0 for a valid pack
- [ ] `avix package validate ./bad-dir` exits 1 and prints each error with its file path
- [ ] `avix package build ./agents/packs/universal-tool-explorer --version v0.1.0` creates `universal-tool-explorer-v0.1.0.tar.xz` + `checksums.sha256`
- [ ] `avix package build ./services/workspace --version v1.0.0` creates `workspace-v1.0.0-linux-x86_64.tar.xz` (platform-suffixed)
- [ ] Built archive decompresses correctly and contains all source files (excluding `.git/`, `target/`, `Cargo.lock`)
- [ ] `sha256sum -c checksums.sha256` passes against the built archives
- [ ] `avix package new my-agent --type agent` creates a valid agent scaffold that passes `validate`
- [ ] `avix package new my-svc --type service` creates a valid service scaffold that passes `validate`
- [ ] All commands work without a running Avix server
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
