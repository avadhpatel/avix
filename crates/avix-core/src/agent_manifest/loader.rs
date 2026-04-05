use std::sync::Arc;

use sha2::{Digest, Sha256};
use tracing::warn;

use super::schema::AgentManifest;
use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

pub struct ManifestLoader {
    pub(crate) vfs: Arc<VfsRouter>,
}

impl ManifestLoader {
    pub fn new(vfs: Arc<VfsRouter>) -> Self {
        Self { vfs }
    }

    /// Load a manifest for a named agent, returning the manifest and its package directory.
    ///
    /// Resolution order:
    ///   1. `/bin/<name>@<version>/manifest.yaml`  (system-installed, any version)
    ///   2. `/users/<username>/bin/<name>@<version>/manifest.yaml`  (user-installed, any version)
    pub async fn load_with_dir(
        &self,
        name: &str,
        username: &str,
    ) -> Result<(AgentManifest, String), AvixError> {
        if let Some(path) = self.find_versioned_manifest("/bin", name).await {
            let pkg_dir = path.trim_end_matches("/manifest.yaml").to_string();
            return self.load_from_path(&path).await.map(|m| (m, pkg_dir));
        }

        let user_bin = format!("/users/{}/bin", username);
        if let Some(path) = self.find_versioned_manifest(&user_bin, name).await {
            let pkg_dir = path.trim_end_matches("/manifest.yaml").to_string();
            return self.load_from_path(&path).await.map(|m| (m, pkg_dir));
        }

        Err(AvixError::ManifestNotFound {
            path: format!("/bin/{}/manifest.yaml", name),
        })
    }

    /// Load a manifest for a named agent.
    pub async fn load(&self, name: &str, username: &str) -> Result<AgentManifest, AvixError> {
        self.load_with_dir(name, username).await.map(|(m, _)| m)
    }

    /// Find a manifest in a versioned directory (e.g., /bin/researcher@1.0.0/)
    async fn find_versioned_manifest(&self, base_dir: &str, name: &str) -> Option<String> {
        let dir = match VfsPath::parse(base_dir) {
            Ok(p) => p,
            Err(_) => return None,
        };

        let entries = match self.vfs.list(&dir).await {
            Ok(e) => e,
            Err(_) => return None,
        };

        for entry in entries {
            if let Some(versioned_name) = entry.strip_prefix(&format!("{}@", name)) {
                if !versioned_name.is_empty() {
                    let manifest_path = format!("{}/{}/manifest.yaml", base_dir, entry);
                    if let Ok(path) = VfsPath::parse(&manifest_path) {
                        if self.vfs.exists(&path).await {
                            return Some(manifest_path);
                        }
                    }
                }
            }
        }
        None
    }

    /// Load from an exact VFS path.
    pub async fn load_from_path(&self, path: &str) -> Result<AgentManifest, AvixError> {
        let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let raw = self
            .vfs
            .read(&vfs_path)
            .await
            .map_err(|_| AvixError::ManifestNotFound {
                path: path.to_string(),
            })?;
        let manifest = AgentManifest::from_yaml(
            std::str::from_utf8(&raw).map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        )?;
        if manifest.kind != "Agent" {
            return Err(AvixError::ManifestKindMismatch {
                expected: "Agent".into(),
                found: manifest.kind,
            });
        }
        Self::verify_signature(&raw, &manifest, path)?;
        Ok(manifest)
    }

    /// Verify the manifest's `packaging.signature` against a SHA-256 hash of its
    /// canonical YAML content.
    ///
    /// If the signature is absent or `"sha256:"` (empty hex), verification is skipped
    /// with a warning — this is the dev/test sentinel value.
    pub fn verify_signature(
        _raw_yaml: &[u8],
        manifest: &AgentManifest,
        path: &str,
    ) -> Result<(), AvixError> {
        let sig = manifest.packaging.signature.as_deref().unwrap_or("");
        let hex_part = sig.strip_prefix("sha256:").unwrap_or("");
        if hex_part.is_empty() {
            warn!(path, "signature verification skipped for dev manifest");
            return Ok(());
        }
        let mut canonical = manifest.clone();
        canonical.packaging.signature = Some("sha256:".to_string());
        let canonical_yaml =
            serde_yaml::to_string(&canonical).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let hash = Sha256::digest(canonical_yaml.as_bytes());
        let computed = hex::encode(hash);
        if computed != hex_part {
            return Err(AvixError::ManifestSignatureMismatch {
                path: path.to_string(),
            });
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const ECHO_BOT_YAML: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: echo-bot
  version: 1.0.0
  description: Simple echo agent
  author: avix-core
packaging:
  signature: "sha256:"
spec:
  systemPromptPath: system-prompt.md
  entrypoint:
    type: llm-loop
"#;

    async fn vfs_with_manifest(path: &str, yaml: &str) -> Arc<VfsRouter> {
        let vfs = Arc::new(VfsRouter::new());
        let vfs_path = VfsPath::parse(path).unwrap();
        vfs.write(&vfs_path, yaml.as_bytes().to_vec())
            .await
            .unwrap();
        vfs
    }

    // T-MGB-08
    #[tokio::test]
    async fn loader_loads_manifest_from_vfs() {
        let vfs = vfs_with_manifest("/bin/echo-bot@1.0.0/manifest.yaml", ECHO_BOT_YAML).await;
        let loader = ManifestLoader::new(vfs);
        let manifest = loader
            .load_from_path("/bin/echo-bot@1.0.0/manifest.yaml")
            .await
            .unwrap();
        assert_eq!(manifest.metadata.name, "echo-bot");
        assert_eq!(manifest.metadata.version, "1.0.0");
    }

    // T-MGB-09
    #[tokio::test]
    async fn loader_returns_not_found_for_missing_manifest() {
        let vfs = Arc::new(VfsRouter::new());
        let loader = ManifestLoader::new(vfs);
        let result = loader
            .load_from_path("/bin/nonexistent@1.0.0/manifest.yaml")
            .await;
        assert!(matches!(result, Err(AvixError::ManifestNotFound { .. })));
    }

    // T-MGB-10
    #[tokio::test]
    async fn loader_rejects_wrong_kind() {
        let wrong_kind = r#"
apiVersion: avix/v1
kind: SomethingElse
metadata:
  name: x
  version: 1.0.0
spec: {}
"#;
        let vfs = vfs_with_manifest("/bin/x@1.0.0/manifest.yaml", wrong_kind).await;
        let loader = ManifestLoader::new(vfs);
        let result = loader.load_from_path("/bin/x@1.0.0/manifest.yaml").await;
        assert!(matches!(
            result,
            Err(AvixError::ManifestKindMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn loader_resolves_system_path_first() {
        let vfs = vfs_with_manifest("/bin/echo-bot@1.0.0/manifest.yaml", ECHO_BOT_YAML).await;
        let loader = ManifestLoader::new(vfs);
        let manifest = loader.load("echo-bot", "alice").await.unwrap();
        assert_eq!(manifest.metadata.name, "echo-bot");
    }

    #[tokio::test]
    async fn loader_falls_back_to_user_path() {
        let vfs = vfs_with_manifest(
            "/users/alice/bin/echo-bot@1.0.0/manifest.yaml",
            ECHO_BOT_YAML,
        )
        .await;
        let loader = ManifestLoader::new(vfs);
        let manifest = loader.load("echo-bot", "alice").await.unwrap();
        assert_eq!(manifest.metadata.name, "echo-bot");
    }

    #[tokio::test]
    async fn loader_returns_not_found_when_neither_path_exists() {
        let vfs = Arc::new(VfsRouter::new());
        let loader = ManifestLoader::new(vfs);
        let result = loader.load("missing-agent", "alice").await;
        assert!(matches!(result, Err(AvixError::ManifestNotFound { .. })));
    }

    #[tokio::test]
    async fn load_with_dir_returns_package_directory() {
        let vfs = vfs_with_manifest("/bin/echo-bot@1.0.0/manifest.yaml", ECHO_BOT_YAML).await;
        let loader = ManifestLoader::new(vfs);
        let (manifest, pkg_dir) = loader.load_with_dir("echo-bot", "alice").await.unwrap();
        assert_eq!(manifest.metadata.name, "echo-bot");
        assert_eq!(pkg_dir, "/bin/echo-bot@1.0.0");
    }
}
