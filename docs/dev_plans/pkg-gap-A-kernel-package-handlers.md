# pkg-gap-A — Kernel Package Handlers

> **Status:** Done (incorporated into docs/architecture/15-packaging.md)
> **Priority:** Critical — all other packaging gaps depend on this
> **Depends on:** nothing (xz2 crate addition required)
> **Blocks:** pkg-gap-B, pkg-gap-C
> **Affects:**
> - `crates/avix-core/src/service/installer.rs` (xz support + agent install path)
> - `crates/avix-core/src/syscall/domain/proc_.rs` (new `install_agent` + `install_service` handlers)
> - `crates/avix-core/src/syscall/handler.rs` (route `proc/package/install-*`)
> - `crates/avix-core/src/syscall/registry.rs` (register new syscalls)
> - `Cargo.toml` (add `xz2`)

---

## Problem

The existing `ServiceInstaller` only handles `.tar.gz` archives and has no agent install path.
There is no `proc/package/install-agent` or `proc/package/install-service` kernel syscall, no
capability enforcement for installs, no `github:` source resolution, no session audit logging
of installs, and no automatic `ManifestScanner::scan_all()` refresh after agent install.

---

## Scope

This gap delivers:
1. **xz decompression** — replace gzip with xz in `extract_tarball`.
2. **`PackageSource` resolver** — canonicalize `github:`, `git:`, `https://`, `file://`, and bare paths.
3. **`AgentInstaller`** — parallel to `ServiceInstaller` but for agent packs.
4. **`proc/package/install-agent`** and **`proc/package/install-service`** syscalls.
5. **Capability enforcement** (`install:agent`, `install:service`, `install:from-untrusted-source`).
6. **Session audit log** — write a `MessageRecord` + `PartRecord` to `HistoryStore` after each install.
7. **Post-install hooks** — refresh `ManifestScanner` after agent install; call `ServiceInstaller::register`
   after service install.

No CLI wiring (gap B). No TUI/Web-UI (gap C). No GPG (gap D).

---

## What to Build

### 1. Add `xz2` to workspace `Cargo.toml`

```toml
xz2 = { version = "0.1" }
```

Add `xz2.workspace = true` to `crates/avix-core/Cargo.toml`.

### 2. `PackageSource` — `crates/avix-core/src/service/package_source.rs`

```rust
use crate::error::AvixError;

/// Canonical package source after resolution.
#[derive(Debug, Clone)]
pub enum PackageSource {
    /// `https://…tar.xz` (already a direct URL)
    HttpUrl(String),
    /// `file:///absolute/path` or `/absolute/path`
    LocalPath(std::path::PathBuf),
    /// Resolved GitHub Releases URL (after `github:` resolution).
    GitHubRelease { url: String, checksum_url: Option<String> },
    /// `git:https://…` — clone HEAD of default branch.
    GitClone(String),
}

impl PackageSource {
    /// Parse and resolve a user-supplied source string.
    ///
    /// Patterns:
    /// - `github:owner/repo/name[@version]` → latest GitHub Release asset URL
    /// - `github.com/owner/name` → same
    /// - `git:https://…` → GitClone
    /// - `https://…` / `http://…` → HttpUrl
    /// - `file://…` / `/…` / `./…` → LocalPath
    pub async fn resolve(source: &str, version: Option<&str>) -> Result<Self, AvixError> {
        if let Some(spec) = source.strip_prefix("github:") {
            return Self::resolve_github(spec, version).await;
        }
        if source.starts_with("github.com/") {
            return Self::resolve_github(source.trim_start_matches("github.com/"), version).await;
        }
        if let Some(repo_url) = source.strip_prefix("git:") {
            return Ok(Self::GitClone(repo_url.to_owned()));
        }
        if source.starts_with("https://") || source.starts_with("http://") {
            return Ok(Self::HttpUrl(source.to_owned()));
        }
        if let Some(path) = source.strip_prefix("file://") {
            return Ok(Self::LocalPath(std::path::PathBuf::from(path)));
        }
        if source.starts_with('/') || source.starts_with("./") || source.starts_with("../") {
            return Ok(Self::LocalPath(std::path::PathBuf::from(source)));
        }
        Err(AvixError::ConfigParse(format!("unrecognized source: {source}")))
    }

    /// Resolve `owner/repo/name[@version]` to a GitHub Releases asset URL.
    ///
    /// Uses the GitHub API: `GET /repos/{owner}/{repo}/releases/latest`
    /// (or `/releases/tags/{version}` when version is specified).
    /// Finds the asset whose name matches `{name}-{version}-{os}-{arch}.tar.xz`
    /// or `{name}-{version}.tar.xz` as fallback.
    async fn resolve_github(spec: &str, version: Option<&str>) -> Result<Self, AvixError> {
        // spec: "owner/repo/name" — repo defaults to "avix" when only "owner/name"
        let parts: Vec<&str> = spec.splitn(3, '/').collect();
        let (owner, repo, name) = match parts.as_slice() {
            [owner, repo, name] => (*owner, *repo, *name),
            [owner, name] => (*owner, "avix", *name),
            _ => {
                return Err(AvixError::ConfigParse(format!(
                    "invalid github: source '{spec}'"
                )))
            }
        };

        let tag = version.unwrap_or("latest");
        let api_url = if tag == "latest" {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}")
        };

        let client = reqwest::Client::builder()
            .user_agent("avix-installer/0.1")
            .build()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let release: serde_json::Value = client
            .get(&api_url)
            .send()
            .await
            .map_err(|e| AvixError::ConfigParse(format!("GitHub API: {e}")))?
            .json()
            .await
            .map_err(|e| AvixError::ConfigParse(format!("GitHub API json: {e}")))?;

        let resolved_version = release["tag_name"]
            .as_str()
            .unwrap_or(tag)
            .trim_start_matches('v');
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        // Asset name candidates: platform-specific first, then generic.
        let candidates = [
            format!("{name}-v{resolved_version}-{os}-{arch}.tar.xz"),
            format!("{name}-v{resolved_version}.tar.xz"),
            format!("{name}-{resolved_version}.tar.xz"),
        ];

        let assets = release["assets"].as_array().ok_or_else(|| {
            AvixError::ConfigParse("GitHub release has no assets".into())
        })?;

        let mut asset_url = None;
        let mut checksum_url = None;
        for candidate in &candidates {
            for asset in assets {
                let asset_name = asset["name"].as_str().unwrap_or("");
                if asset_name == candidate {
                    asset_url = Some(asset["browser_download_url"].as_str().unwrap_or("").to_owned());
                }
                if asset_name == "checksums.sha256" {
                    checksum_url = Some(asset["browser_download_url"].as_str().unwrap_or("").to_owned());
                }
            }
            if asset_url.is_some() {
                break;
            }
        }

        let url = asset_url.ok_or_else(|| {
            AvixError::ConfigParse(format!(
                "no matching asset for '{name}' in GitHub release {tag}"
            ))
        })?;

        Ok(Self::GitHubRelease { url, checksum_url })
    }
}
```

### 3. Update `ServiceInstaller::extract_tarball` for `.tar.xz`

Replace the `flate2::GzDecoder` in `installer.rs` with xz2 decoding. The function should detect
the format from the source URL extension or try xz first:

```rust
pub fn extract_tarball(&self, bytes: &[u8], dest: &Path) -> Result<(), AvixError> {
    // Try xz first (primary format), fall back to gzip for backward compat.
    let reader: Box<dyn std::io::Read> = if Self::is_xz(bytes) {
        Box::new(xz2::read::XzDecoder::new(bytes))
    } else {
        Box::new(flate2::read::GzDecoder::new(bytes))
    };
    let mut archive = tar::Archive::new(reader);
    // … rest unchanged except manifest file check accepts both service.yaml and manifest.yaml
}

fn is_xz(bytes: &[u8]) -> bool {
    bytes.len() >= 6 && &bytes[..6] == b"\xfd7zXZ\x00"
}
```

Also update the manifest file check: accept `manifest.yaml` (agents) in addition to `service.yaml` (services).
Set `found_unit = true` for either.

### 4. `AgentManifestFile` — `crates/avix-core/src/agent_manifest/manifest_file.rs`

Minimal typed struct for the `manifest.yaml` inside an agent pack:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifestFile {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt_path: Option<String>,
    #[serde(default)]
    pub examples: Vec<String>,
}
```

### 5. `AgentInstaller` — `crates/avix-core/src/agent_manifest/installer.rs`

```rust
pub struct AgentInstallRequest {
    pub source: String,
    pub version: Option<String>,
    pub scope: InstallScope,
    pub checksum: Option<String>,
    pub session_id: Option<uuid::Uuid>,
    pub no_verify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallScope {
    System,  // → /bin/<name>/
    User(String), // → /users/<username>/bin/<name>/
}

pub struct AgentInstaller {
    root: std::path::PathBuf,
}

impl AgentInstaller {
    pub fn new(root: std::path::PathBuf) -> Self { Self { root } }

    pub async fn install(&self, req: AgentInstallRequest) -> Result<AgentInstallResult, AvixError> {
        let pkg_source = PackageSource::resolve(&req.source, req.version.as_deref()).await?;
        let bytes = self.fetch_source(&pkg_source).await?;

        if !req.no_verify {
            if let Some(expected) = &req.checksum {
                ServiceInstaller::static_verify_checksum(&bytes, expected)?;
            } else if let PackageSource::GitHubRelease { checksum_url: Some(url), .. } = &pkg_source {
                self.fetch_and_verify_checksum_file(&bytes, url).await?;
            }
        }

        let tmp = tempfile::tempdir()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let extractor = ServiceInstaller::new(self.root.clone());
        extractor.extract_tarball(&bytes, tmp.path())?;

        let manifest = AgentManifestFile::load(&tmp.path().join("manifest.yaml"))?;
        let install_dir = match &req.scope {
            InstallScope::System => self.root.join("bin").join(&manifest.name),
            InstallScope::User(u) => self.root.join("users").join(u).join("bin").join(&manifest.name),
        };
        if install_dir.exists() {
            return Err(AvixError::ConfigParse(format!(
                "agent already installed: {}", manifest.name
            )));
        }
        std::fs::create_dir_all(&install_dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        // copy all extracted files into install_dir
        copy_dir_all(tmp.path(), &install_dir)?;

        Ok(AgentInstallResult {
            name: manifest.name,
            version: manifest.version,
            install_dir,
        })
    }
}

pub struct AgentInstallResult {
    pub name: String,
    pub version: String,
    pub install_dir: std::path::PathBuf,
}
```

Implement `AgentManifestFile::load(path: &Path) -> Result<Self, AvixError>` in `manifest_file.rs`.

### 6. New syscall handlers — `crates/avix-core/src/syscall/domain/pkg_.rs`

```rust
use serde_json::{json, Value};
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

/// `proc/package/install-agent`
///
/// Required capability: `install:agent`.
/// For non-GitHub, non-official sources: also requires `install:from-untrusted-source`.
pub async fn install_agent(ctx: &SyscallContext, params: Value, /* deps */) -> SyscallResult {
    check_capability(ctx, "install:agent")?;

    let source = params["source"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    check_untrusted_if_needed(ctx, source)?;

    let scope = parse_scope(&params, &ctx.username)?;
    let version = params["version"].as_str()
        .filter(|v| *v != "latest")
        .map(|v| v.to_owned());
    let checksum = params["checksum"].as_str().map(|s| s.to_owned());
    let no_verify = params["no_verify"].as_bool().unwrap_or(false);
    let session_id = params["session_id"].as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    let req = AgentInstallRequest { source: source.to_owned(), version, scope, checksum, session_id, no_verify };
    let result = agent_installer.install(req).await
        .map_err(|e| SyscallError::Eio(e.to_string()))?;

    // Trigger ManifestScanner refresh (non-blocking).
    tokio::spawn(async move { manifest_scanner.scan_all().await; });

    // Log to HistoryStore if session_id provided.
    if let Some(sid) = session_id {
        log_install_to_session(&history_store, sid, "agent", &result.name, source).await;
    }

    Ok(json!({
        "name": result.name,
        "version": result.version,
        "install_dir": result.install_dir.display().to_string()
    }))
}

/// `proc/package/install-service`
///
/// Required capability: `install:service`.
pub async fn install_service(ctx: &SyscallContext, params: Value, /* deps */) -> SyscallResult {
    check_capability(ctx, "install:service")?;

    let source = params["source"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    check_untrusted_if_needed(ctx, source)?;

    let version = params["version"].as_str()
        .filter(|v| *v != "latest")
        .map(|v| v.to_owned());
    let checksum = params["checksum"].as_str().map(|s| s.to_owned());
    let no_verify = params["no_verify"].as_bool().unwrap_or(false);
    let session_id = params["session_id"].as_str()
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    // Resolve source → bytes → extract → register.
    let pkg_source = PackageSource::resolve(source, version.as_deref()).await
        .map_err(|e| SyscallError::Einval(e.to_string()))?;
    let bytes = fetch_source_bytes(&pkg_source).await
        .map_err(|e| SyscallError::Eio(e.to_string()))?;

    if !no_verify {
        if let Some(ck) = &checksum {
            ServiceInstaller::static_verify_checksum(&bytes, ck)
                .map_err(|e| SyscallError::Einval(e.to_string()))?;
        }
    }

    let req = InstallRequest { source: source.to_owned(), checksum, autostart: false };
    let result = service_installer.install(req).await
        .map_err(|e| SyscallError::Eio(e.to_string()))?;

    // Auto-register service with router via existing ipc/register path.
    register_installed_service(&service_registry, &result).await
        .map_err(|e| SyscallError::Eio(e.to_string()))?;

    if let Some(sid) = session_id {
        log_install_to_session(&history_store, sid, "service", &result.name, source).await;
    }

    Ok(json!({
        "name": result.name,
        "version": result.version,
        "install_dir": result.install_dir.display().to_string()
    }))
}

fn check_capability(ctx: &SyscallContext, cap: &str) -> Result<(), SyscallError> {
    if !ctx.token.granted_tools.iter().any(|t| t == cap) {
        return Err(SyscallError::Eperm(format!("missing capability: {cap}")));
    }
    Ok(())
}

fn check_untrusted_if_needed(ctx: &SyscallContext, source: &str) -> Result<(), SyscallError> {
    let is_official = source.contains("github.com/avadhpatel/avix")
        || source.starts_with("github:avadhpatel/avix");
    if !is_official {
        check_capability(ctx, "install:from-untrusted-source")?;
    }
    Ok(())
}

fn parse_scope(params: &Value, username: &str) -> Result<InstallScope, SyscallError> {
    match params["scope"].as_str().unwrap_or("user") {
        "system" => Ok(InstallScope::System),
        "user" | _ => Ok(InstallScope::User(username.to_owned())),
    }
}
```

### 7. Session audit log helper

```rust
async fn log_install_to_session(
    store: &HistoryStore,
    session_id: uuid::Uuid,
    kind: &str,          // "agent" | "service"
    name: &str,
    source: &str,
) {
    let msg_id = uuid::Uuid::new_v4();
    let msg = MessageRecord {
        id: msg_id,
        session_id,
        role: Role::System,
        created_at: chrono::Utc::now(),
        ..Default::default()
    };
    let part = PartRecord::text(
        msg_id,
        0,
        &format!("Installed {kind} `{name}` from `{source}`"),
    );
    let _ = store.append_message(msg).await;
    let _ = store.append_part(part).await;
}
```

### 8. Wire into `syscall/handler.rs` and `syscall/registry.rs`

In `handler.rs`, add routes:
```
"proc/package/install-agent"  → pkg_::install_agent(ctx, params, deps).await
"proc/package/install-service" → pkg_::install_service(ctx, params, deps).await
```

In `registry.rs`, register:
```
"proc/package/install-agent"
"proc/package/install-service"
```

Both syscalls are async and require the `SyscallContext` to carry the `CapabilityToken` (already present).

### 9. Wire into ATP gateway

In `gateway.svc` or wherever ATP `cmd` messages are dispatched, add forwarding for the two new `proc/package/*` ops. These follow the same `ipc_forward` pattern used for `proc/list-installed`, etc.

---

## Tests

All tests go in `#[cfg(test)]` blocks in the respective modules.

### `package_source.rs`
- `resolve_https_url()` — `https://…` stays as `HttpUrl`
- `resolve_local_abs_path()` — `/abs/path` → `LocalPath`
- `resolve_local_rel_path()` — `./rel` → `LocalPath`
- `resolve_file_scheme()` — `file:///abs` → `LocalPath`
- `resolve_git_clone()` — `git:https://…` → `GitClone`
- `resolve_github_two_part()` — `github:owner/name` → owner=owner, repo="avix", name=name
- `resolve_github_three_part()` — `github:owner/repo/name` → correct parts
- `resolve_unknown_scheme_errors()` — `ftp://…` → `Err`

### `service/installer.rs`
- `is_xz_magic()` — detects xz magic bytes correctly
- `is_not_xz_gz()` — gzip bytes return false
- `extract_xz_tarball()` — create a real `.tar.xz` in a tempdir, extract, check files present
- `extract_gz_tarball_backward_compat()` — `.tar.gz` still works

### `agent_manifest/installer.rs`
- `install_agent_local_path()` — pack a test agent `.tar.xz` with `manifest.yaml`, install to user scope, verify files in place
- `install_agent_system_scope()` — install to `/bin/` scope
- `install_agent_conflict_errors()` — re-install same name → `Err`
- `install_agent_no_manifest_errors()` — archive without `manifest.yaml` → `Err`

### `syscall/domain/pkg_.rs`
- `missing_install_capability_eperm()` — token without `install:agent` → `Eperm`
- `untrusted_source_without_cap_eperm()` — `https://example.com/…` without `install:from-untrusted-source` → `Eperm`
- `official_source_no_untrusted_cap_ok()` — `github:avadhpatel/avix/…` without `install:from-untrusted-source` → succeeds

---

## Success Criteria

- [ ] `.tar.xz` archives extract correctly; `.tar.gz` still works (backward compat)
- [ ] `PackageSource::resolve` handles all 6 input patterns
- [ ] `github:owner/repo/name` resolves to a real GitHub Releases URL (integration test with mock HTTP)
- [ ] `AgentInstaller` installs an agent pack to user and system scopes
- [ ] `proc/package/install-agent` and `proc/package/install-service` syscalls are registered and routable
- [ ] Capability checks enforce `install:agent`, `install:service`, `install:from-untrusted-source`
- [ ] Session audit log entry written when `session_id` provided
- [ ] `ManifestScanner::scan_all()` is triggered after agent install
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
