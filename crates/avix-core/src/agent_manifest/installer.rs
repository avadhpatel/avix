use std::path::PathBuf;

use crate::agent_manifest::git_fetch::git_clone_to;
use crate::agent_manifest::manifest_file::AgentManifestFile;
use crate::error::AvixError;
use crate::service::installer::ServiceInstaller;
use crate::service::package_source::PackageSource;

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
    System,
    User(String),
}

pub struct AgentInstaller {
    root: PathBuf,
}

struct InstallGuard {
    path: PathBuf,
    committed: bool,
}

impl InstallGuard {
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

impl Drop for InstallGuard {
    fn drop(&mut self) {
        if !self.committed && self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

impl AgentInstaller {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn install(&self, req: AgentInstallRequest) -> Result<AgentInstallResult, AvixError> {
        let pkg_source = PackageSource::resolve(&req.source, req.version.as_deref()).await?;

        let (tmp, manifest) = match &pkg_source {
            PackageSource::GitClone(url) => {
                let tmp = tempfile::tempdir().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                git_clone_to(url, tmp.path()).await?;
                let manifest_path = tmp.path().join("manifest.yaml");
                let manifest = AgentManifestFile::load(&manifest_path)?;
                (Some(tmp), manifest)
            }
            _ => {
                let bytes = self.fetch_source(&pkg_source).await?;

                if !req.no_verify {
                    if let Some(expected) = &req.checksum {
                        ServiceInstaller::static_verify_checksum(&bytes, expected)?;
                    } else if let PackageSource::GitHubRelease {
                        checksum_url: Some(url),
                        ..
                    } = &pkg_source
                    {
                        self.fetch_and_verify_checksum_file(&bytes, url).await?;
                    }
                }

                let tmp = tempfile::tempdir().map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let extractor = ServiceInstaller::new(self.root.clone());
                extractor.extract_tarball(&bytes, tmp.path())?;

                let manifest = AgentManifestFile::load(&tmp.path().join("manifest.yaml"))?;
                (Some(tmp), manifest)
            }
        };

        // Use versioned directory name: <name>@<version>
        let versioned_name = format!("{}@{}", manifest.name, manifest.version);
        
        let install_dir = match &req.scope {
            InstallScope::System => self.root.join("data").join("bin").join(&versioned_name),
            InstallScope::User(u) => self
                .root
                .join("data")
                .join("users")
                .join(u)
                .join("bin")
                .join(&versioned_name),
        };
        
        // Check if this specific version is already installed
        if install_dir.exists() {
            return Err(AvixError::ConfigParse(format!(
                "agent version already installed: {}@{}",
                manifest.name, manifest.version
            )));
        }

        // Also check if a different version of the same agent exists (for potential upgrade path)
        let base_dir = match &req.scope {
            InstallScope::System => self.root.join("data").join("bin"),
            InstallScope::User(u) => self.root.join("data").join("users").join(u).join("bin"),
        };
        
        // Find any existing versions of this agent
        if let Ok(entries) = std::fs::read_dir(&base_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with(&format!("{}@", manifest.name)) {
                            tracing::debug!("found existing version of {}: {}", manifest.name, name);
                        }
                    }
                }
            }
        }

        let mut guard = InstallGuard::new(install_dir.clone());
        std::fs::create_dir_all(&install_dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        if let Some(tmp) = tmp {
            if let Err(e) = std::fs::rename(tmp.path(), &install_dir) {
                drop(tmp);
                let _ = std::fs::remove_dir_all(&install_dir);
                return Err(AvixError::ConfigParse(format!(
                    "install failed and rolled back: {}",
                    e
                )));
            }
        }

        guard.commit();
        Ok(AgentInstallResult {
            name: manifest.name,
            version: manifest.version,
            install_dir,
        })
    }

    async fn fetch_source(&self, source: &PackageSource) -> Result<Vec<u8>, AvixError> {
        match source {
            PackageSource::HttpUrl(url) => {
                let bytes = reqwest::get(url)
                    .await
                    .map_err(|e| AvixError::ConfigParse(format!("fetch {}: {}", url, e)))?
                    .bytes()
                    .await
                    .map_err(|e| AvixError::ConfigParse(format!("fetch body {}: {}", url, e)))?;
                Ok(bytes.to_vec())
            }
            PackageSource::LocalPath(path) => std::fs::read(path)
                .map_err(|e| AvixError::ConfigParse(format!("read {}: {}", path.display(), e))),
            PackageSource::GitHubRelease { url, .. } => {
                let bytes = reqwest::get(url)
                    .await
                    .map_err(|e| AvixError::ConfigParse(format!("fetch {}: {}", url, e)))?
                    .bytes()
                    .await
                    .map_err(|e| AvixError::ConfigParse(format!("fetch body {}: {}", url, e)))?;
                Ok(bytes.to_vec())
            }
            PackageSource::GitClone(_) => {
                unreachable!("GitClone is handled in install() method")
            }
        }
    }

    async fn fetch_and_verify_checksum_file(
        &self,
        bytes: &[u8],
        url: &str,
    ) -> Result<(), AvixError> {
        let checksum_content = reqwest::get(url)
            .await
            .map_err(|e| AvixError::ConfigParse(format!("fetch checksum {}: {}", url, e)))?
            .text()
            .await
            .map_err(|e| AvixError::ConfigParse(format!("read checksum {}: {}", url, e)))?;

        let hex = checksum_content
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next())
            .ok_or_else(|| AvixError::ConfigParse("invalid checksum file".into()))?;

        ServiceInstaller::static_verify_checksum(bytes, &format!("sha256:{}", hex))
    }
}

pub struct AgentInstallResult {
    pub name: String,
    pub version: String,
    pub install_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_xz_tarball(name: &str, version: &str) -> Vec<u8> {
        use xz2::write::XzEncoder;
        let mut buf = Vec::new();
        {
            let enc = XzEncoder::new(&mut buf, 6);
            let mut ar = tar::Builder::new(enc);

            let manifest_content = format!(
                "name: {}\nversion: {}\ndescription: Test agent\n",
                name, version
            );
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest_content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                format!("{}-{}/manifest.yaml", name, version),
                manifest_content.as_bytes(),
            )
            .unwrap();

            ar.finish().unwrap();
        }
        buf
    }

    #[tokio::test]
    async fn install_agent_local_path() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_xz_tarball("test-agent", "1.0.0");
        let pkg_path = dir.path().join("test-agent-1.0.0.tar.xz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = AgentInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(AgentInstallRequest {
                source: format!("file://{}", pkg_path.display()),
                version: None,
                scope: InstallScope::User("alice".to_string()),
                checksum: None,
                session_id: None,
                no_verify: false,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "test-agent");
        assert_eq!(result.version, "1.0.0");
        assert!(root
            .path()
            .join("data/users/alice/bin/test-agent@1.0.0/manifest.yaml")
            .exists());
    }

    #[tokio::test]
    async fn install_agent_system_scope() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_xz_tarball("system-agent", "1.0.0");
        let pkg_path = dir.path().join("system-agent-1.0.0.tar.xz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = AgentInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(AgentInstallRequest {
                source: format!("file://{}", pkg_path.display()),
                version: None,
                scope: InstallScope::System,
                checksum: None,
                session_id: None,
                no_verify: false,
            })
            .await
            .unwrap();

        assert_eq!(result.name, "system-agent");
        assert!(root.path().join("data/bin/system-agent@1.0.0/manifest.yaml").exists());
    }

    #[tokio::test]
    async fn install_agent_conflict_errors() {
        let dir = TempDir::new().unwrap();
        let pkg_bytes = make_xz_tarball("conflict-agent", "1.0.0");
        let pkg_path = dir.path().join("conflict-agent-1.0.0.tar.xz");
        std::fs::write(&pkg_path, &pkg_bytes).unwrap();

        let root = TempDir::new().unwrap();
        let installer = AgentInstaller::new(root.path().to_path_buf());

        installer
            .install(AgentInstallRequest {
                source: format!("file://{}", pkg_path.display()),
                version: None,
                scope: InstallScope::User("alice".to_string()),
                checksum: None,
                session_id: None,
                no_verify: false,
            })
            .await
            .unwrap();

        let result = installer
            .install(AgentInstallRequest {
                source: format!("file://{}", pkg_path.display()),
                version: None,
                scope: InstallScope::User("alice".to_string()),
                checksum: None,
                session_id: None,
                no_verify: false,
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn install_agent_no_manifest_errors() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let dir = TempDir::new().unwrap();
        let mut buf = Vec::new();
        {
            let enc = GzEncoder::new(&mut buf, Compression::default());
            let mut ar = tar::Builder::new(enc);

            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, "pkg-1.0/readme.txt", b"hello".as_ref())
                .unwrap();
            ar.finish().unwrap();
        }
        let pkg_path = dir.path().join("no-manifest.tar.gz");
        std::fs::write(&pkg_path, &buf).unwrap();

        let root = TempDir::new().unwrap();
        let installer = AgentInstaller::new(root.path().to_path_buf());
        let result = installer
            .install(AgentInstallRequest {
                source: format!("file://{}", pkg_path.display()),
                version: None,
                scope: InstallScope::User("alice".to_string()),
                checksum: None,
                session_id: None,
                no_verify: false,
            })
            .await;
        assert!(result.is_err());
    }
}
