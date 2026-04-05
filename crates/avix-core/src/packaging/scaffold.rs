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
            "name: {}\nversion: \"{}\"\ndescription: \"\"\nsystem_prompt_path: system-prompt.md\n",
            name, version
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
            dir.join("service.yaml"),
            format!(
                r#"name: "{}"
version: "{}"

unit:
  description: ""
  after: ["router.svc"]

service:
  binary: "/services/{name}/bin/{name}"
  language: "rust"
  restart: "on-failure"

capabilities:
  caller_scoped: false

tools:
  namespace: "/tools/{name}/"
  provides: []
"#,
                name, version
            ),
        )
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                r#"[package]
name    = "{}"
version = "{}"
edition = "2021"

[[bin]]
name = "{}"
path = "src/main.rs"
"#,
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
        assert!(result.join("service.yaml").exists());
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
        assert_eq!(parsed["name"].as_str(), Some("test-agent"));
    }

    #[test]
    fn scaffold_service_unit_is_valid_yaml() {
        let dir = TempDir::new().unwrap();
        let req = ScaffoldRequest {
            name: "test-svc".into(),
            pkg_type: PackageType::Service,
            version: "0.1.0".into(),
            output_dir: dir.path().to_path_buf(),
        };

        let result = PackageScaffold::create(req).unwrap();
        let content = std::fs::read_to_string(result.join("service.yaml")).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert_eq!(parsed["name"].as_str(), Some("test-svc"));
    }
}
