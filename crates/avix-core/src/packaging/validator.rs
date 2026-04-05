use super::PackageType;
use crate::agent_manifest::AgentManifestFile;
use crate::service::ServiceUnit;
use std::path::Path;

#[derive(Debug)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

pub struct PackageValidator;

impl PackageValidator {
    pub fn validate(dir: &Path) -> Result<PackageType, Vec<ValidationError>> {
        let pkg_type = PackageType::detect(dir).map_err(|e| {
            vec![ValidationError {
                path: dir.display().to_string(),
                message: e.to_string(),
            }]
        })?;
        let mut errors = Vec::new();
        match pkg_type {
            PackageType::Agent => Self::validate_agent(dir, &mut errors),
            PackageType::Service => Self::validate_service(dir, &mut errors),
        }
        if errors.is_empty() {
            Ok(pkg_type)
        } else {
            Err(errors)
        }
    }

    fn validate_agent(dir: &Path, errors: &mut Vec<ValidationError>) {
        let manifest_path = dir.join("manifest.yaml");
        match std::fs::read_to_string(&manifest_path) {
            Err(e) => errors.push(ValidationError {
                path: "manifest.yaml".into(),
                message: format!("cannot read: {e}"),
            }),
            Ok(content) => {
                if let Err(e) = serde_yaml::from_str::<AgentManifestFile>(&content) {
                    errors.push(ValidationError {
                        path: "manifest.yaml".into(),
                        message: format!("parse error: {e}"),
                    });
                } else {
                    let m: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or_default();
                    if m["name"].as_str().unwrap_or("").is_empty() {
                        errors.push(ValidationError {
                            path: "manifest.yaml".into(),
                            message: "name is empty".into(),
                        });
                    }
                    if m["version"].as_str().unwrap_or("").is_empty() {
                        errors.push(ValidationError {
                            path: "manifest.yaml".into(),
                            message: "version is empty".into(),
                        });
                    }
                    if let Some(prompt_path) = m["system_prompt_path"].as_str() {
                        if !dir.join(prompt_path).exists() {
                            errors.push(ValidationError {
                                path: prompt_path.into(),
                                message: "system_prompt_path references missing file".into(),
                            });
                        }
                    }
                }
            }
        }
    }

    fn validate_service(dir: &Path, errors: &mut Vec<ValidationError>) {
        let unit_path = dir.join("service.yaml");
        match std::fs::read_to_string(&unit_path) {
            Err(e) => errors.push(ValidationError {
                path: "service.yaml".into(),
                message: format!("cannot read: {e}"),
            }),
            Ok(content) => {
                if let Err(e) = serde_yaml::from_str::<ServiceUnit>(&content) {
                    errors.push(ValidationError {
                        path: "service.yaml".into(),
                        message: format!("parse error: {e}"),
                    });
                }
            }
        }
        let bin_dir = dir.join("bin");
        if !bin_dir.exists() {
            errors.push(ValidationError {
                path: "bin/".into(),
                message: "bin/ directory is missing (build the binary first)".into(),
            });
        } else {
            let has_binary = std::fs::read_dir(&bin_dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
            if !has_binary {
                errors.push(ValidationError {
                    path: "bin/".into(),
                    message: "bin/ directory is empty".into(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detect_agent_from_manifest_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("manifest.yaml"),
            "name: test\nversion: 0.1.0\n",
        )
        .unwrap();

        let result = PackageType::detect(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Agent);
    }

    #[test]
    fn detect_service_from_service_unit() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("service.yaml"), "name: test\n").unwrap();

        let result = PackageType::detect(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Service);
    }

    #[test]
    fn detect_unknown_errors() {
        let dir = TempDir::new().unwrap();

        let result = PackageType::detect(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn valid_agent_pack_passes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("manifest.yaml"),
            "name: test\nversion: \"0.1.0\"\nsystem_prompt_path: system-prompt.md\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("system-prompt.md"), "# Test\n").unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Agent);
    }

    #[test]
    fn agent_missing_manifest_errors() {
        let dir = TempDir::new().unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn agent_empty_name_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("manifest.yaml"),
            "name: \"\"\nversion: \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("system-prompt.md"), "# Test\n").unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("empty")));
    }

    #[test]
    fn agent_missing_prompt_file_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("manifest.yaml"),
            "name: test\nversion: \"0.1.0\"\nsystem_prompt_path: nonexistent.md\n",
        )
        .unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("missing file")));
    }

    #[test]
    fn valid_service_passes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("service.yaml"), "name: test\nversion: \"0.1.0\"\n\nunit:\n  description: \"\"\n\nservice:\n  binary: \"/bin/test\"\n  language: \"rust\"\n\ntools:\n  namespace: \"/tools/test/\"\n  provides: []\n").unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/test"), "").unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Service);
    }

    #[test]
    fn service_missing_unit_errors() {
        let dir = TempDir::new().unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn service_missing_bin_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("service.yaml"), "name: test\nversion: \"0.1.0\"\n\nunit:\n  description: \"\"\n\nservice:\n  binary: \"/bin/test\"\n  language: \"rust\"\n\ntools:\n  namespace: \"/tools/test/\"\n  provides: []\n").unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.path == "bin/"));
    }

    #[test]
    fn service_empty_bin_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("service.yaml"), "name: test\nversion: \"0.1.0\"\n\nunit:\n  description: \"\"\n\nservice:\n  binary: \"/bin/test\"\n  language: \"rust\"\n\ntools:\n  namespace: \"/tools/test/\"\n  provides: []\n").unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("empty")));
    }
}
