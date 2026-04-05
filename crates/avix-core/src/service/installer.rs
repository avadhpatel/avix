use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::AvixError;
use crate::service::install_receipt::InstallReceipt;
use crate::service::yaml::ServiceUnit;

pub struct InstallRequest {
    /// `file:///absolute/path` or `https://…`
    pub source: String,
    /// Optional expected checksum in `"sha256:<hex>"` format.
    pub checksum: Option<String>,
    /// If true, caller should spawn the service after installation.
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

struct ServiceInstallGuard {
    path: PathBuf,
    committed: bool,
}

impl ServiceInstallGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ServiceInstallGuard {
    fn drop(&mut self) {
        if !self.committed && self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

impl ServiceInstaller {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn install(&self, req: InstallRequest) -> Result<InstallResult, AvixError> {
        let bytes = self.fetch(&req.source).await?;

        if let Some(expected) = &req.checksum {
            self.verify_checksum(&bytes, expected)?;
        }

        let tmp_dir = tempfile::tempdir().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.extract_tarball(&bytes, tmp_dir.path())?;

        let unit = ServiceUnit::load(&tmp_dir.path().join("manifest.yaml"))?;

        self.check_conflicts(&unit)?;

        // Use versioned directory name: <name>@<version>
        let versioned_name = format!("{}@{}", unit.name, unit.version);
        let install_dir = self
            .root
            .join("data")
            .join("services")
            .join(&versioned_name);

        let mut guard = ServiceInstallGuard::new(install_dir.clone());
        std::fs::create_dir_all(&install_dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        if let Err(e) = std::fs::rename(tmp_dir.path(), &install_dir) {
            drop(tmp_dir);
            let _ = std::fs::remove_dir_all(&install_dir);
            return Err(AvixError::ConfigParse(format!(
                "install failed and rolled back: {}",
                e
            )));
        }

        let receipt = InstallReceipt {
            name: unit.name.clone(),
            version: unit.version.clone(),
            source_url: Some(req.source.clone()),
            checksum: req.checksum.clone(),
            installed_at: chrono::Utc::now(),
            service_unit_path: install_dir.join("manifest.yaml").display().to_string(),
            binary_path: unit.service.binary.clone(),
        };
        let receipt_path = install_dir.join(".install.json");
        let json = serde_json::to_string_pretty(&receipt)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(&receipt_path, json).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        guard.commit();
        Ok(InstallResult {
            name: unit.name.clone(),
            version: unit.version.clone(),
            install_dir,
            receipt_path,
            tools: unit.tools.provides.clone(),
        })
    }

    async fn fetch(&self, source: &str) -> Result<Vec<u8>, AvixError> {
        if let Some(path) = source.strip_prefix("file://") {
            std::fs::read(path)
                .map_err(|e| AvixError::ConfigParse(format!("cannot read {path}: {e}")))
        } else if source.starts_with("https://") || source.starts_with("http://") {
            let bytes = reqwest::get(source)
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch {source}: {e}")))?
                .bytes()
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch body {source}: {e}")))?;
            Ok(bytes.to_vec())
        } else {
            Err(AvixError::ConfigParse(format!(
                "unsupported source scheme: {source}"
            )))
        }
    }

    pub fn verify_checksum(&self, bytes: &[u8], expected: &str) -> Result<(), AvixError> {
        Self::static_verify_checksum(bytes, expected)
    }

    pub fn static_verify_checksum(bytes: &[u8], expected: &str) -> Result<(), AvixError> {
        let (algo, expected_hex) = expected.split_once(':').ok_or_else(|| {
            AvixError::ConfigParse(format!("invalid checksum format: {expected}"))
        })?;
        match algo {
            "sha256" => {
                let digest = hex::encode(Sha256::digest(bytes));
                if digest != expected_hex {
                    return Err(AvixError::ConfigParse(format!(
                        "checksum mismatch: expected {expected_hex} got {digest}"
                    )));
                }
                Ok(())
            }
            other => Err(AvixError::ConfigParse(format!(
                "unsupported checksum algorithm: {other}"
            ))),
        }
    }

    pub fn extract_tarball(&self, bytes: &[u8], dest: &Path) -> Result<(), AvixError> {
        let reader: Box<dyn std::io::Read> = if Self::is_xz(bytes) {
            Box::new(xz2::read::XzDecoder::new(bytes))
        } else {
            Box::new(flate2::read::GzDecoder::new(bytes))
        };
        let mut archive = tar::Archive::new(reader);

        let mut found_unit = false;
        for entry in archive
            .entries()
            .map_err(|e| AvixError::ConfigParse(format!("tarball read: {e}")))?
        {
            let mut entry =
                entry.map_err(|e| AvixError::ConfigParse(format!("tarball entry: {e}")))?;
            let raw_path = entry
                .path()
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?
                .to_path_buf();

            let stripped = raw_path.components().skip(1).collect::<PathBuf>();

            if stripped.as_os_str().is_empty() {
                continue;
            }

            let out_path = dest.join(&stripped);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            }

            if entry.header().entry_type().is_file() {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                std::fs::write(&out_path, &data)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

                #[cfg(unix)]
                if stripped.starts_with("bin") {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&out_path, perms)
                        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                }

                if stripped == Path::new("manifest.yaml") {
                    found_unit = true;
                }
            }
        }

        if !found_unit {
            return Err(AvixError::ConfigParse(
                "tarball missing required manifest.yaml file".into(),
            ));
        }
        Ok(())
    }

    fn is_xz(bytes: &[u8]) -> bool {
        bytes.len() >= 6 && &bytes[..6] == b"\xfd7zXZ\x00"
    }

    pub fn check_conflicts(&self, unit: &ServiceUnit) -> Result<(), AvixError> {
        // Check for this specific version
        let versioned_name = format!("{}@{}", unit.name, unit.version);
        let existing = self
            .root
            .join("data")
            .join("services")
            .join(&versioned_name);
        if existing.exists() {
            return Err(AvixError::ConfigParse(format!(
                "service version already installed: {}@{}",
                unit.name, unit.version
            )));
        }

        // Also check for any other versions of this service
        let services_dir = self.root.join("data").join("services");
        if let Ok(entries) = std::fs::read_dir(&services_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with(&format!("{}@", unit.name)) {
                            tracing::debug!("found existing version of {}: {}", unit.name, name);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_tarball(name: &str, version: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut ar = tar::Builder::new(enc);

            let unit_content = format!(
                "apiVersion: avix/v1\nkind: Service\nmetadata:\n  name: {name}\n  version: {version}\n  description: test\nspec:\n  binary: /services/{name}/bin/{name}\n  tools:\n    namespace: /tools/{name}/\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(unit_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                format!("{name}-{version}/manifest.yaml"),
                unit_content.as_bytes(),
            )
            .unwrap();

            let enc = ar.into_inner().unwrap();
            enc.finish().unwrap();
        }
        buf
    }

    fn make_test_unit(name: &str) -> ServiceUnit {
        use crate::service::yaml::{ServiceSection, ToolsSection, UnitSection};
        ServiceUnit {
            name: name.into(),
            version: "1.0.0".into(),
            source: crate::service::yaml::ServiceSource::User,
            signature: None,
            unit: UnitSection::default(),
            service: ServiceSection {
                binary: format!("/services/{name}/bin/{name}"),
                language: "any".into(),
                restart: Default::default(),
                restart_delay: "5s".into(),
                max_concurrent: 20,
                queue_max: 100,
                queue_timeout: "5s".into(),
                run_as: Default::default(),
            },
            capabilities: Default::default(),
            tools: ToolsSection {
                namespace: format!("/tools/{name}/"),
                provides: vec![],
            },
            jobs: Default::default(),
        }
    }

    #[test]
    fn verify_checksum_passes_for_correct_hash() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let data = b"hello world";
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
    fn verify_checksum_rejects_missing_colon() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        assert!(installer.verify_checksum(b"x", "sha256deadbeef").is_err());
    }

    #[test]
    fn extract_tarball_creates_service_unit() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_tarball("echo-svc", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        assert!(dest.path().join("manifest.yaml").exists());
    }

    #[test]
    fn extract_tarball_strips_top_level_dir() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_tarball("echo-svc", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        // Should NOT have echo-svc-1.0.0/ prefix
        assert!(!dest.path().join("echo-svc-1.0.0").exists());
    }

    #[test]
    fn extract_tarball_fails_on_missing_service_unit() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        // Build a tarball with no manifest.yaml
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut ar = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, "pkg-1.0/readme.txt", b"hello".as_ref())
                .unwrap();
            let enc = ar.into_inner().unwrap();
            enc.finish().unwrap();
        }
        let dest = TempDir::new().unwrap();
        assert!(installer.extract_tarball(&buf, dest.path()).is_err());
    }

    #[test]
    fn check_conflicts_errors_if_already_installed() {
        let dir = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join("data").join("services").join("echo-svc@1.0.0")).unwrap();
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
        let result = installer
            .install(InstallRequest {
                source: format!("file://{}", pkg_path.display()),
                checksum: None,
                autostart: false,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "test-svc");
        assert_eq!(result.version, "1.0.0");
        assert!(root.path().join("data/services/test-svc@1.0.0/manifest.yaml").exists());
        assert!(root.path().join("data/services/test-svc@1.0.0/.install.json").exists());
    }

    #[tokio::test]
    async fn install_with_correct_checksum() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_tarball("cs-svc", "2.0.0");
        let pkg_path = dir.path().join("cs-svc.tar.gz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let checksum = format!("sha256:{}", hex::encode(Sha256::digest(&pkg_bytes)));
        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(InstallRequest {
                source: format!("file://{}", pkg_path.display()),
                checksum: Some(checksum),
                autostart: false,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "cs-svc");
        assert!(root.path().join("data/services/cs-svc@2.0.0/.install.json").exists());
    }

    #[tokio::test]
    async fn install_fails_on_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_tarball("test-svc", "1.0.0");
        let pkg_path = dir.path().join("test-svc.tar.gz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(InstallRequest {
                source: format!("file://{}", pkg_path.display()),
                checksum: Some("sha256:badbadbad".into()),
                autostart: false,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn install_fails_on_unsupported_scheme() {
        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(InstallRequest {
                source: "ftp://example.com/pkg.tar.gz".into(),
                checksum: None,
                autostart: false,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn install_receipt_contains_correct_fields() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_tarball("receipt-svc", "3.0.0");
        let pkg_path = dir.path().join("receipt-svc.tar.gz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = ServiceInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(InstallRequest {
                source: format!("file://{}", pkg_path.display()),
                checksum: None,
                autostart: false,
            })
            .await
            .unwrap();

        let json = std::fs::read_to_string(&result.receipt_path).unwrap();
        let receipt: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(receipt["name"], "receipt-svc");
        assert_eq!(receipt["version"], "3.0.0");
    }

    fn make_xz_tarball(name: &str, version: &str) -> Vec<u8> {
        use xz2::write::XzEncoder;
        let mut buf = Vec::new();
        {
            let enc = XzEncoder::new(&mut buf, 6);
            let mut ar = tar::Builder::new(enc);

            let unit_content = format!(
                "apiVersion: avix/v1\nkind: Service\nmetadata:\n  name: {name}\n  version: {version}\n  description: test\nspec:\n  binary: /services/{name}/bin/{name}\n  tools:\n    namespace: /tools/{name}/\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(unit_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                format!("{name}-{version}/manifest.yaml"),
                unit_content.as_bytes(),
            )
            .unwrap();

            ar.finish().unwrap();
        }
        buf
    }

    #[test]
    fn is_xz_magic() {
        let xz_bytes = b"\xfd7zXZ\x00test";
        assert!(ServiceInstaller::is_xz(xz_bytes));
    }

    #[test]
    fn is_not_xz_gz() {
        let gz_bytes = [0x1f, 0x8b, 0x08, 0x00, 0x00];
        assert!(!ServiceInstaller::is_xz(&gz_bytes));
    }

    #[tokio::test]
    async fn extract_xz_tarball() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_xz_tarball("xz-svc", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        assert!(dest.path().join("manifest.yaml").exists());
    }

    #[tokio::test]
    async fn extract_gz_tarball() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_tarball("gz-svc", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        assert!(dest.path().join("manifest.yaml").exists());
    }

    fn make_xz_tarball_with_manifest(name: &str, version: &str) -> Vec<u8> {
        use xz2::write::XzEncoder;
        let mut buf = Vec::new();
        {
            let enc = XzEncoder::new(&mut buf, 6);
            let mut ar = tar::Builder::new(enc);

            let manifest_content = format!(
                "apiVersion: avix/v1\nkind: Service\nmetadata:\n  name: {name}\n  version: {version}\n  description: test agent\nspec:\n  binary: /bin/{name}\n  tools:\n    namespace: /tools/{name}/\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                format!("{name}-{version}/manifest.yaml"),
                manifest_content.as_bytes(),
            )
            .unwrap();

            ar.finish().unwrap();
        }
        buf
    }

    #[tokio::test]
    async fn extract_xz_tarball_with_manifest_yaml() {
        let installer = ServiceInstaller::new(PathBuf::from("/tmp"));
        let bytes = make_xz_tarball_with_manifest("test-agent", "1.0.0");
        let dest = TempDir::new().unwrap();
        installer.extract_tarball(&bytes, dest.path()).unwrap();
        assert!(dest.path().join("manifest.yaml").exists());
    }
}
