use super::PackageType;
use crate::error::AvixError;
use std::path::PathBuf;

pub struct ScaffoldRequest {
    pub name: String,
    pub pkg_type: PackageType,
    pub version: String,
    pub output_dir: PathBuf,
}

pub struct PackageScaffold;

impl PackageScaffold {
    pub fn create(req: ScaffoldRequest) -> Result<PathBuf, AvixError> {
        let dir = req.output_dir.join(&req.name);
        if dir.exists() {
            return Err(AvixError::ConfigParse(format!(
                "directory already exists: {}",
                dir.display()
            )));
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
        std::fs::write(
            dir.join("manifest.yaml"),
            format!(
                "apiVersion: avix/v1\nkind: Agent\nmetadata:\n  name: {name}\n  version: \"{version}\"\n  description: \"\"\nspec:\n  systemPromptPath: system-prompt.md\n"
            ),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("system-prompt.md"),
            format!("# {}\n\nYou are a helpful agent.\n", name),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("README.md"),
            format!("# {}\n\nDescribe your agent here.\n", name),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }

    fn scaffold_service(dir: &std::path::Path, name: &str, version: &str) -> Result<(), AvixError> {
        std::fs::create_dir_all(dir.join("src"))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::create_dir_all(dir.join("tools"))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("manifest.yaml"),
            format!(
                "apiVersion: avix/v1\nkind: Service\nmetadata:\n  name: {name}\n  version: \"{version}\"\n  description: \"\"\nspec:\n  binary: \"/services/{name}/bin/{name}\"\n  language: rust\n  restart: on-failure\n  after:\n    - router.svc\n  capabilities:\n    callerScoped: false\n  tools:\n    namespace: \"/tools/{name}/\"\n    provides: []\n"
            ),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname    = \"{}\"\nversion = \"{}\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{}\"\npath = \"src/main.rs\"\n",
                name, version, name
            ),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("src/main.rs"),
            format!("fn main() {{\n    println!(\"Hello from {}\");\n}}\n", name),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("README.md"),
            format!("# {}\n\nDescribe your service here.\n", name),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scaffold_agent_creates_required_files() {
        let dir = TempDir::new().unwrap();
        let req = ScaffoldRequest {
            name: "test-agent".into(),
            pkg_type: PackageType::Agent,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req).unwrap();
        assert!(result.join("manifest.yaml").exists());
        assert!(result.join("system-prompt.md").exists());
        assert!(result.join("README.md").exists());
    }

    #[test]
    fn scaffold_service_creates_required_files() {
        let dir = TempDir::new().unwrap();
        let req = ScaffoldRequest {
            name: "test-svc".into(),
            pkg_type: PackageType::Service,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req).unwrap();
        assert!(result.join("manifest.yaml").exists());
        assert!(result.join("Cargo.toml").exists());
        assert!(result.join("src/main.rs").exists());
    }

    #[test]
    fn scaffold_existing_dir_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("existing")).unwrap();

        let req = ScaffoldRequest {
            name: "existing".into(),
            pkg_type: PackageType::Agent,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req);
        assert!(result.is_err());
    }

    #[test]
    fn scaffold_agent_manifest_is_valid_yaml() {
        let dir = TempDir::new().unwrap();
        let req = ScaffoldRequest {
            name: "test-agent".into(),
            pkg_type: PackageType::Agent,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req).unwrap();
        let content = std::fs::read_to_string(result.join("manifest.yaml")).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert_eq!(parsed["kind"].as_str(), Some("Agent"));
        assert_eq!(parsed["metadata"]["name"].as_str(), Some("test-agent"));
    }

    #[test]
    fn scaffold_service_manifest_is_valid_yaml() {
        let dir = TempDir::new().unwrap();
        let req = ScaffoldRequest {
            name: "test-svc".into(),
            pkg_type: PackageType::Service,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req).unwrap();
        let content = std::fs::read_to_string(result.join("manifest.yaml")).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert_eq!(parsed["kind"].as_str(), Some("Service"));
        assert_eq!(parsed["metadata"]["name"].as_str(), Some("test-svc"));
    }
}
