# Svc Gap D — Service Installer (Download, Verify, Extract, Receipt)

> **Status:** Pending
> **Priority:** High
> **Depends on:** Svc gap A (`ServiceUnit`, `InstallReceipt` types)
> **Blocks:** Svc gap E (CLI commands)
> **Affects:** `crates/avix-core/src/service/installer.rs` (new),
>   `crates/avix-core/src/syscall/domain/` (new `sys` domain handler)

---

## Problem

There is no `sys/install` ATP command handler and no `ServiceInstaller`. The spec
(`service-authoring.md §9`) and architecture doc (`07-services.md § Service Installation`)
define a 10-step installation flow. None of it exists.

---

## Scope

Implement the full installation pipeline as a pure-Rust function in `avix-core`:
download (or copy local), SHA-256 checksum verify, tarball extract, conflict check,
write `service.unit` + `.install.json` receipt, and (optionally) spawn the process.
Wire it to the `sys/install` syscall. No GUI, no CLI (gap E handles that).

---

## What Needs to Be Built

### 1. `service/installer.rs`

```rust
use std::path::{Path, PathBuf};
use crate::error::AvixError;
use crate::service::unit::ServiceUnit;
use crate::service::install_receipt::InstallReceipt;

pub struct InstallRequest {
    /// `file:///absolute/path/to/pkg.tar.gz` or `https://...`
    pub source: String,
    /// Optional expected checksum — "sha256:<hex>"
    pub checksum: Option<String>,
    /// If true, spawn the service after successful install.
    pub autostart: bool,
}

pub struct InstallResult {
    pub name: String,
    pub version: String,
    pub install_dir: PathBuf,
    pub receipt_path: PathBuf,
    pub tools: Vec<String>,
}

pub struct ServiceInstaller {
    root: PathBuf,
}

impl ServiceInstaller {
    pub fn new(root: PathBuf) -> Self { Self { root } }

    pub async fn install(&self, req: InstallRequest) -> Result<InstallResult, AvixError> {
        // Step 1: Fetch the package bytes (file:// or https://)
        let bytes = self.fetch(&req.source).await?;

        // Step 2: Verify checksum if provided
        if let Some(expected) = &req.checksum {
            self.verify_checksum(&bytes, expected)?;
        }

        // Step 3: Extract tarball to a temp dir, validate structure
        let tmp_dir = tempfile::tempdir()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.extract_tarball(&bytes, tmp_dir.path())?;

        // Step 4: Parse service.unit from extracted package
        let unit = ServiceUnit::load(&tmp_dir.path().join("service.unit"))?;

        // Step 5: Conflict check
        self.check_conflicts(&unit)?;

        // Step 6: Install to AVIX_ROOT/services/<name>/
        let install_dir = self.root.join("services").join(&unit.name);
        self.copy_to_install_dir(tmp_dir.path(), &install_dir)?;

        // Step 7: Write .install.json receipt
        let receipt = InstallReceipt {
            name: unit.name.clone(),
            version: unit.version.clone(),
            source_url: Some(req.source.clone()),
            checksum: req.checksum.clone(),
            installed_at: chrono::Utc::now(),
            service_unit_path: install_dir.join("service.unit").display().to_string(),
            binary_path: unit.service.binary.clone(),
        };
        let receipt_path = install_dir.join(".install.json");
        let json = serde_json::to_string_pretty(&receipt)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(&receipt_path, json)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(InstallResult {
            name: unit.name.clone(),
            version: unit.version.clone(),
            install_dir,
            receipt_path,
            tools: unit.tools.provides.clone(),
        })
    }

    async fn fetch(&self, source: &str) -> Result<Vec<u8>, AvixError> { ... }

    fn verify_checksum(&self, bytes: &[u8], expected: &str) -> Result<(), AvixError> { ... }

    fn extract_tarball(&self, bytes: &[u8], dest: &Path) -> Result<(), AvixError> { ... }

    fn check_conflicts(&self, unit: &ServiceUnit) -> Result<(), AvixError> { ... }

    fn copy_to_install_dir(&self, src: &Path, dest: &Path) -> Result<(), AvixError> { ... }
}
```

#### `fetch`

- `file:///path` → `std::fs::read(path)`
- `https://` → `reqwest::get(url).await?.bytes().await?`
- Anything else → `AvixError::ConfigParse("unsupported source scheme")`

#### `verify_checksum`

- Parse `"sha256:<hex>"` — error if format is wrong
- Compute `sha2::Sha256` digest of bytes
- Compare hex strings (constant-time via `subtle::ConstantTimeEq` or just `==` on hex strings for now)
- Return `AvixError::ConfigParse("checksum mismatch: expected ... got ...")` on failure

#### `extract_tarball`

- Decode `flate2::read::GzDecoder`, parse with `tar::Archive`
- Strip the top-level directory from paths (package is `<name>-<ver>/...`)
- Write extracted files to `dest/`
- Error if `service.unit` is not present in the archive

#### `check_conflicts`

- If `AVIX_ROOT/services/<name>/` already exists → `AvixError::ConfigParse("service already installed: <name>")`
- (Future: check tool namespace conflicts — skip for now, just name check)

#### `copy_to_install_dir`

- `std::fs::create_dir_all(dest)`
- Recursively copy `src/` to `dest/` (use `walkdir` or manual recursion)
- Set executable bit on `bin/*` files on Unix

---

### 2. Syscall handler — `sys/install`

Add to `crates/avix-core/src/syscall/domain/`:

```rust
// syscall/domain/sys.rs

pub async fn handle_install(
    ctx: &SyscallContext,
    body: serde_json::Value,
    installer: &ServiceInstaller,
) -> SyscallResult {
    // Verify caller has admin token
    if !ctx.token.grants.contains("auth:admin") {
        return Err(SyscallError::Eperm(ctx.caller_pid, "sys/install".into()));
    }
    let source = body["source"].as_str()
        .ok_or_else(|| SyscallError::Einval("missing `source`".into()))?
        .to_string();
    let checksum = body["checksum"].as_str().map(String::from);
    let autostart = body["autostart"].as_bool().unwrap_or(true);

    let result = installer
        .install(InstallRequest { source, checksum, autostart })
        .await
        .map_err(|e| SyscallError::Einval(e.to_string()))?;

    Ok(serde_json::json!({
        "name":    result.name,
        "version": result.version,
        "tools":   result.tools,
        "install_dir": result.install_dir.display().to_string(),
    }))
}
```

---

## Dependencies to add to `avix-core/Cargo.toml`

```toml
sha2       = "0.10"
flate2     = "1"
tar        = "0.4"
walkdir    = "2"
# reqwest already present in avix-client-core; add to avix-core if not there:
reqwest    = { version = "0.12", features = ["rustls-tls"], default-features = false, optional = true }
```

---

## Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tarball(name: &str, version: &str) -> Vec<u8> {
        // Build a minimal in-memory tar.gz with:
        //   <name>-<ver>/service.unit
        //   <name>-<ver>/bin/<name>
        use std::io::Write;
        let mut buf = Vec::new();
        let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut ar = tar::Builder::new(enc);
        // write service.unit
        let unit_content = format!(
            "name=\"{name}\"\nversion=\"{version}\"\n\
             [unit]\n[service]\nbinary=\"/services/{name}/bin/{name}\"\n\
             [tools]\nnamespace=\"/tools/{name}/\"\n"
        );
        let mut header = tar::Header::new_gnu();
        header.set_size(unit_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        ar.append_data(&mut header,
            format!("{name}-{version}/service.unit"),
            unit_content.as_bytes()).unwrap();
        let enc = ar.into_inner().unwrap();
        enc.finish().unwrap();
        buf
    }

    #[test]
    fn verify_checksum_passes_for_correct_hash() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let data = b"hello world";
        use sha2::{Sha256, Digest};
        let hex = format!("sha256:{}", hex::encode(Sha256::digest(data)));
        assert!(installer.verify_checksum(data, &hex).is_ok());
    }

    #[test]
    fn verify_checksum_fails_for_wrong_hash() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let result = installer.verify_checksum(b"hello", "sha256:deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn verify_checksum_rejects_unknown_algorithm() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let result = installer.verify_checksum(b"hello", "md5:deadbeef");
        assert!(result.is_err());
    }

    #[test]
    fn extract_tarball_creates_service_unit() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_tarball("echo-svc", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        assert!(dest.path().join("service.unit").exists());
    }

    #[test]
    fn check_conflicts_errors_if_already_installed() {
        let dir = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join("services").join("echo-svc")).unwrap();
        let unit = make_test_unit("echo-svc");
        assert!(installer.check_conflicts(&unit).is_err());
    }

    #[test]
    fn check_conflicts_ok_when_not_installed() {
        let dir = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(dir.path().to_path_buf());
        let unit = make_test_unit("new-svc");
        assert!(installer.check_conflicts(&unit).is_ok());
    }

    #[tokio::test]
    async fn install_from_local_file() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_tarball("test-svc", "1.0.0");
        let pkg_path = dir.path().join("test-svc-1.0.0.tar.gz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer.install(InstallRequest {
            source: format!("file://{}", pkg_path.display()),
            checksum: None,
            autostart: false,
        }).await.unwrap();

        assert_eq!(result.name, "test-svc");
        assert!(root.path().join("services/test-svc/service.unit").exists());
        assert!(root.path().join("services/test-svc/.install.json").exists());
    }

    #[tokio::test]
    async fn install_fails_on_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_tarball("test-svc", "1.0.0");
        let pkg_path = dir.path().join("test-svc.tar.gz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer.install(InstallRequest {
            source: format!("file://{}", pkg_path.display()),
            checksum: Some("sha256:badbadbad".into()),
            autostart: false,
        }).await;

        assert!(result.is_err());
    }
}
```

---

## Success Criteria

- [ ] `verify_checksum` passes on correct SHA-256, fails on wrong hash, errors on unknown algorithm
- [ ] `extract_tarball` strips top-level dir and writes `service.unit` to dest
- [ ] `check_conflicts` errors when service directory already exists
- [ ] `install` from `file://` creates the install dir, `service.unit`, and `.install.json`
- [ ] `install` fails if checksum doesn't match
- [ ] `sys/install` syscall handler enforces `auth:admin` capability
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
