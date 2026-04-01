use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::schema::AgentManifest;
use crate::memfs::{VfsPath, VfsRouter};

// ── AgentScope ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentScope {
    /// Installed in `/bin/` — available to all users.
    System,
    /// Installed in `/users/<username>/bin/` — available only to that user.
    User,
}

// ── AgentManifestSummary ──────────────────────────────────────────────────────

/// Lightweight summary of an installed agent manifest.
///
/// Returned by `ManifestScanner::scan()` for catalog and discovery operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifestSummary {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    /// VFS path to the manifest file, e.g. `/bin/researcher/manifest.yaml`.
    pub path: String,
    pub scope: AgentScope,
}

// ── ManifestScanner ───────────────────────────────────────────────────────────

/// Scans VFS directories to enumerate installed agent manifests.
///
/// Resolution order:
///   1. `/bin/` (System scope) — available to all users.
///   2. `/users/<username>/bin/` (User scope) — personal installs.
///
/// When a user-installed agent has the same `name` as a system agent, the system
/// agent takes precedence and the user entry is omitted from the result.
pub struct ManifestScanner {
    vfs: Arc<VfsRouter>,
}

impl ManifestScanner {
    pub fn new(vfs: Arc<VfsRouter>) -> Self {
        Self { vfs }
    }

    /// List all agents available to `username` — system + user-installed.
    pub async fn scan(&self, username: &str) -> Vec<AgentManifestSummary> {
        let mut results = self.scan_dir("/bin", AgentScope::System).await;
        let system_names: std::collections::HashSet<String> =
            results.iter().map(|s| s.name.clone()).collect();

        let user_dir = format!("/users/{}/bin", username);
        let user = self.scan_dir(&user_dir, AgentScope::User).await;
        for entry in user {
            if !system_names.contains(&entry.name) {
                results.push(entry);
            }
        }
        results
    }

    /// Admin variant: scans `/bin/` plus all `/users/*/bin/` directories.
    pub async fn scan_all(&self) -> Vec<AgentManifestSummary> {
        let mut results = self.scan_dir("/bin", AgentScope::System).await;
        let system_names: std::collections::HashSet<String> =
            results.iter().map(|s| s.name.clone()).collect();
        let mut seen: std::collections::HashSet<String> = system_names.clone();

        let users_path = match VfsPath::parse("/users") {
            Ok(p) => p,
            Err(_) => return results,
        };
        let usernames = self.vfs.list(&users_path).await.unwrap_or_default();
        for username in &usernames {
            if username.starts_with('.') {
                continue;
            }
            let user_dir = format!("/users/{}/bin", username);
            let user = self.scan_dir(&user_dir, AgentScope::User).await;
            for entry in user {
                if !seen.contains(&entry.name) {
                    seen.insert(entry.name.clone());
                    results.push(entry);
                }
            }
        }
        results
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Enumerate all subdirectories of `dir_path` and attempt to load
    /// `manifest.yaml` from each. Entries that fail to parse are skipped with
    /// a `warn!()`.
    async fn scan_dir(&self, dir_path: &str, scope: AgentScope) -> Vec<AgentManifestSummary> {
        let dir = match VfsPath::parse(dir_path) {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let entries = match self.vfs.list(&dir).await {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        let mut summaries = Vec::new();
        for entry in &entries {
            if entry.starts_with('.') {
                continue; // skip .keep anchors and hidden files
            }
            let manifest_path = format!("{}/{}/manifest.yaml", dir_path, entry);
            let vfs_path = match VfsPath::parse(&manifest_path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let raw = match self.vfs.read(&vfs_path).await {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let yaml_str = match std::str::from_utf8(&raw) {
                Ok(s) => s,
                Err(e) => {
                    warn!(path = %manifest_path, "manifest is not valid UTF-8: {e}");
                    continue;
                }
            };
            let manifest = match AgentManifest::from_yaml(yaml_str) {
                Ok(m) => m,
                Err(e) => {
                    warn!(path = %manifest_path, "failed to parse manifest: {e}");
                    continue;
                }
            };
            if manifest.kind != "AgentManifest" {
                warn!(path = %manifest_path, kind = %manifest.kind, "unexpected manifest kind, skipping");
                continue;
            }
            summaries.push(AgentManifestSummary {
                name: manifest.metadata.name,
                version: manifest.metadata.version,
                description: manifest.metadata.description,
                author: manifest.metadata.author,
                path: manifest_path,
                scope: scope.clone(),
            });
        }
        summaries
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_YAML: &str = r#"
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: researcher
  version: 1.0.0
  description: Research agent
  author: avix-team
  createdAt: "2026-01-01T00:00:00Z"
  signature: "sha256:"
spec:
  entrypoint:
    type: llm-loop
"#;

    const CODER_YAML: &str = r#"
apiVersion: avix/v1
kind: AgentManifest
metadata:
  name: coder
  version: 2.0.0
  description: Code generation agent
  author: avix-team
  createdAt: "2026-01-01T00:00:00Z"
  signature: "sha256:"
spec:
  entrypoint:
    type: llm-loop
"#;

    async fn make_vfs_with(entries: &[(&str, &str)]) -> Arc<VfsRouter> {
        let vfs = Arc::new(VfsRouter::new());
        for (path, yaml) in entries {
            let p = VfsPath::parse(path).unwrap();
            vfs.write(&p, yaml.as_bytes().to_vec()).await.unwrap();
        }
        vfs
    }

    // T-SCN-01
    #[tokio::test]
    async fn empty_bins_returns_empty() {
        let vfs = Arc::new(VfsRouter::new());
        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan("alice").await;
        assert!(result.is_empty());
    }

    // T-SCN-02
    #[tokio::test]
    async fn two_system_agents_returned() {
        let vfs = make_vfs_with(&[
            ("/bin/researcher/manifest.yaml", MANIFEST_YAML),
            ("/bin/coder/manifest.yaml", CODER_YAML),
        ])
        .await;
        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan("alice").await;
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|s| s.scope == AgentScope::System));
        let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"coder"));
    }

    // T-SCN-03
    #[tokio::test]
    async fn user_agent_returned_with_user_scope() {
        let vfs = make_vfs_with(&[("/users/alice/bin/my-bot/manifest.yaml", MANIFEST_YAML)]).await;
        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan("alice").await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].scope, AgentScope::User);
        assert_eq!(result[0].name, "researcher");
    }

    // T-SCN-04
    #[tokio::test]
    async fn system_wins_on_name_collision() {
        let vfs = make_vfs_with(&[
            ("/bin/researcher/manifest.yaml", MANIFEST_YAML),
            ("/users/alice/bin/researcher/manifest.yaml", MANIFEST_YAML),
        ])
        .await;
        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan("alice").await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].scope, AgentScope::System);
    }

    // T-SCN-05
    #[tokio::test]
    async fn malformed_manifest_is_skipped() {
        let vfs = make_vfs_with(&[
            ("/bin/bad/manifest.yaml", "not: valid: yaml: : :"),
            ("/bin/researcher/manifest.yaml", MANIFEST_YAML),
        ])
        .await;
        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan("alice").await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "researcher");
    }

    // T-SCN-06
    #[tokio::test]
    async fn scan_all_spans_multiple_users() {
        let vfs = make_vfs_with(&[
            ("/bin/researcher/manifest.yaml", MANIFEST_YAML),
            ("/users/alice/bin/coder/manifest.yaml", CODER_YAML),
            ("/users/bob/bin/coder/manifest.yaml", CODER_YAML), // same name as alice's — deduplicated
        ])
        .await;
        // Ensure /users is listable by writing .keep entries
        let keep = VfsPath::parse("/users/.keep").unwrap();
        vfs.write(&keep, b".keep".to_vec()).await.unwrap();
        let alice_keep = VfsPath::parse("/users/alice/.keep").unwrap();
        vfs.write(&alice_keep, b".keep".to_vec()).await.unwrap();
        let bob_keep = VfsPath::parse("/users/bob/.keep").unwrap();
        vfs.write(&bob_keep, b".keep".to_vec()).await.unwrap();

        let scanner = ManifestScanner::new(vfs);
        let result = scanner.scan_all().await;
        // researcher (system) + coder (user — first one wins deduplication)
        assert_eq!(result.len(), 2);
        let names: Vec<&str> = result.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"coder"));
    }
}
