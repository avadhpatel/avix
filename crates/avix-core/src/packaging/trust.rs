use crate::error::AvixError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedKey {
    pub fingerprint: String,
    pub label: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub allowed_sources: Vec<String>,
}

impl TrustedKey {
    pub fn allows_source(&self, source: &str) -> bool {
        if self.allowed_sources.is_empty() {
            return true;
        }
        self.allowed_sources
            .iter()
            .any(|pattern| glob_match(pattern, source))
    }
}

pub struct TrustStore {
    dir: PathBuf,
}

impl TrustStore {
    pub fn new(root: &Path) -> Self {
        Self {
            dir: root.join("etc/avix/trusted-keys"),
        }
    }

    pub fn add(
        &self,
        key_asc: &str,
        label: &str,
        allowed_sources: Vec<String>,
    ) -> Result<TrustedKey, AvixError> {
        std::fs::create_dir_all(&self.dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let fingerprint = extract_fingerprint(key_asc)?;

        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));

        if key_path.exists() {
            return Err(AvixError::ConfigParse(format!(
                "key already trusted: {fingerprint}"
            )));
        }

        let trusted = TrustedKey {
            fingerprint: fingerprint.clone(),
            label: label.to_owned(),
            added_at: chrono::Utc::now(),
            allowed_sources,
        };

        std::fs::write(&key_path, key_asc).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let meta_yaml =
            serde_yaml::to_string(&trusted).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(&meta_path, meta_yaml).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(trusted)
    }

    pub fn list(&self) -> Result<Vec<TrustedKey>, AvixError> {
        if !self.dir.exists() {
            return Ok(vec![]);
        }
        let mut keys = Vec::new();
        for entry in
            std::fs::read_dir(&self.dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?
        {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let key: TrustedKey = serde_yaml::from_str(&content)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                keys.push(key);
            }
        }
        keys.sort_by(|a, b| a.added_at.cmp(&b.added_at));
        Ok(keys)
    }

    pub fn remove(&self, fingerprint: &str) -> Result<(), AvixError> {
        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));
        if !key_path.exists() {
            return Err(AvixError::ConfigParse(format!(
                "key not found: {fingerprint}"
            )));
        }
        std::fs::remove_file(&key_path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::remove_file(&meta_path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    pub fn get(&self, fingerprint: &str) -> Result<Option<(String, TrustedKey)>, AvixError> {
        let key_path = self.dir.join(format!("{fingerprint}.asc"));
        let meta_path = self.dir.join(format!("{fingerprint}.meta.yaml"));
        if !key_path.exists() {
            return Ok(None);
        }
        let key_asc = std::fs::read_to_string(&key_path)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let meta: TrustedKey = serde_yaml::from_str(
            &std::fs::read_to_string(&meta_path)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?,
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(Some((key_asc, meta)))
    }
}

fn extract_fingerprint(key_asc: &str) -> Result<String, AvixError> {
    use pgp::composed::Deserializable;
    use pgp::types::KeyDetails;
    let (pubkey, _) = pgp::composed::SignedPublicKey::from_string(key_asc)
        .map_err(|e| AvixError::ConfigParse(format!("invalid key: {e}")))?;
    Ok(hex::encode(pubkey.fingerprint()).to_uppercase())
}

fn glob_match(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return value.starts_with(&format!("{prefix}/")) || value == prefix;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_KEY_ASC: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----

mQINBF+wM8ABEACrA5W1W1K3K3i1nF6u4T3vY3L4X5X7X8X9X0X1X2X3X4X5X6
X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6
X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6
X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6
X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6X7X8X9X0X1X2X3X4X5X6
=XQ4F
-----END PGP PUBLIC KEY BLOCK-----";

    #[test]
    fn allows_source_empty_patterns_allows_all() {
        let key = TrustedKey {
            fingerprint: "ABC123".to_string(),
            label: "Test".to_string(),
            added_at: chrono::Utc::now(),
            allowed_sources: vec![],
        };
        assert!(key.allows_source("anything"));
        assert!(key.allows_source("github:acmecorp/my-agent"));
    }

    #[test]
    fn allows_source_glob_prefix() {
        let key = TrustedKey {
            fingerprint: "ABC123".to_string(),
            label: "Test".to_string(),
            added_at: chrono::Utc::now(),
            allowed_sources: vec!["github:acmecorp/*".to_string()],
        };
        assert!(key.allows_source("github:acmecorp/my-agent"));
        assert!(key.allows_source("github:acmecorp/other"));
    }

    #[test]
    fn allows_source_glob_no_match() {
        let key = TrustedKey {
            fingerprint: "ABC123".to_string(),
            label: "Test".to_string(),
            added_at: chrono::Utc::now(),
            allowed_sources: vec!["github:acmecorp/*".to_string()],
        };
        assert!(!key.allows_source("github:other/repo"));
        assert!(!key.allows_source("https://packages.example.com"));
    }

    #[test]
    fn allows_source_exact_match() {
        let key = TrustedKey {
            fingerprint: "ABC123".to_string(),
            label: "Test".to_string(),
            added_at: chrono::Utc::now(),
            allowed_sources: vec!["https://packages.example.com/agent.tar.xz".to_string()],
        };
        assert!(key.allows_source("https://packages.example.com/agent.tar.xz"));
        assert!(!key.allows_source("https://packages.example.com/other.tar.xz"));
    }

    #[test]
    fn list_empty_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let store = TrustStore::new(dir.path());
        let keys = store.list().unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let dir = TempDir::new().unwrap();
        let store = TrustStore::new(dir.path());
        let result = store.get("NONEXISTENT").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn remove_nonexistent_errors() {
        let dir = TempDir::new().unwrap();
        let store = TrustStore::new(dir.path());
        let result = store.remove("NONEXISTENT");
        assert!(result.is_err());
    }
}
