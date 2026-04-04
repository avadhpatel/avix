use serde::{Deserialize, Serialize};

use crate::error::AvixError;

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

impl AgentManifestFile {
    pub fn load(path: &std::path::Path) -> Result<Self, AvixError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            AvixError::ConfigParse(format!("cannot read {}: {}", path.display(), e))
        })?;
        serde_yaml::from_str(&content)
            .map_err(|e| AvixError::ConfigParse(format!("parse {}: {}", path.display(), e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_manifest_file() {
        let dir = TempDir::new().unwrap();
        let manifest_path = dir.path().join("manifest.yaml");
        std::fs::write(
            &manifest_path,
            "name: test-agent\nversion: 1.0.0\ndescription: A test agent\n",
        )
        .unwrap();

        let manifest = AgentManifestFile::load(&manifest_path).unwrap();
        assert_eq!(manifest.name, "test-agent");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.description, "A test agent");
    }

    #[test]
    fn load_manifest_missing_error() {
        let dir = TempDir::new().unwrap();
        let result = AgentManifestFile::load(&dir.path().join("nonexistent.yaml"));
        assert!(result.is_err());
    }
}
