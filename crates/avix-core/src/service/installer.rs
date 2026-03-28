use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::AvixError;
use crate::service::install_receipt::InstallReceipt;
use crate::service::unit::ServiceUnit;

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

impl ServiceInstaller {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn install(&self, req: InstallRequest) -> Result<InstallResult, AvixError> {
        let bytes = self.fetch(&req.source).await?;

        if let Some(expected) = &req.checksum {
            self.verify_checksum(&bytes, expected)?;
        }

        let tmp_dir =
            tempfile::tempdir().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.extract_tarball(&bytes, tmp_dir.path())?;

        let unit = ServiceUnit::load(&tmp_dir.path().join("service.unit"))?;

        self.check_conflicts(&unit)?;

        let install_dir = self.root.join("services").join(&unit.name);
        self.copy_to_install_dir(tmp_dir.path(), &install_dir)?;

        let receipt = InstallReceipt {
            name: unit.name.clone(),
            version: unit.version.clone(),
            source_url: Some(req.source.clone()),
            checksum: req.checksum.clone(),
            installed_at: chrono::Utc::now(),
            service_unit_path: install_dir
                .join("service.unit")
                .display()
                .to_string(),
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

    async fn fetch(&self, source: &str) -> Result<Vec<u8>, AvixError> {
        if let Some(path) = source.strip_prefix("file://") {
            std::fs::read(path).map_err(|e| {
                AvixError::ConfigParse(format!("cannot read {path}: {e}"))
            })
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
        let gz = flate2::read::GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(gz);

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

            // Strip the top-level <name>-<version>/ component
            let stripped = raw_path
                .components()
                .skip(1)
                .collect::<PathBuf>();

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

                if stripped == Path::new("service.unit") {
                    found_unit = true;
                }
            }
        }

        if !found_unit {
            return Err(AvixError::ConfigParse(
                "tarball missing required service.unit file".into(),
            ));
        }
        Ok(())
    }

    pub fn check_conflicts(&self, unit: &ServiceUnit) -> Result<(), AvixError> {
        let existing = self.root.join("services").join(&unit.name);
        if existing.exists() {
            return Err(AvixError::ConfigParse(format!(
                "service already installed: {}",
                unit.name
            )));
        }
        Ok(())
    }

    fn copy_to_install_dir(&self, src: &Path, dest: &Path) -> Result<(), AvixError> {
        std::fs::create_dir_all(dest)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        for entry in walkdir::WalkDir::new(src) {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let rel = entry
                .path()
                .strip_prefix(src)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let target = dest.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            } else {
                std::fs::copy(entry.path(), &target)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
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
                "name = \"{name}\"\nversion = \"{version}\"\n\
                 [unit]\n[service]\nbinary = \"/services/{name}/bin/{name}\"\n\
                 [tools]\nnamespace = \"/tools/{name}/\"\n"
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(unit_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                format!("{name}-{version}/service.unit"),
                unit_content.as_bytes(),
            )
            .unwrap();

            let enc = ar.into_inner().unwrap();
            enc.finish().unwrap();
        }
        buf
    }

    fn make_test_unit(name: &str) -> ServiceUnit {
        use crate::service::unit::{ServiceSection, ToolsSection, UnitSection};
        ServiceUnit {
            name: name.into(),
            version: "1.0.0".into(),
            source: crate::service::unit::ServiceSource::User,
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
        assert!(dest.path().join("service.unit").exists());
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
        // Build a tarball with no service.unit
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
        assert!(root
            .path()
            .join("services/test-svc/service.unit")
            .exists());
        assert!(root
            .path()
            .join("services/test-svc/.install.json")
            .exists());
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
        assert!(root.path().join("services/cs-svc/.install.json").exists());
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

        let json =
            std::fs::read_to_string(&result.receipt_path).unwrap();
        let receipt: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(receipt["name"], "receipt-svc");
        assert_eq!(receipt["version"], "3.0.0");
    }
}
