use super::PackageType;
use crate::agent_manifest::AgentManifest;
use crate::service::ServiceManifest;
use std::path::Path;
use tracing::instrument;

#[derive(Debug)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub struct PackageValidator;

impl PackageValidator {
    #[instrument]
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

    #[instrument]
    fn validate_agent(dir: &Path, errors: &mut Vec<ValidationError>) {
        let manifest_path = dir.join("manifest.yaml");
        match std::fs::read_to_string(&manifest_path) {
            Err(e) => errors.push(ValidationError {
                path: "manifest.yaml".into(),
                message: format!("cannot read: {e}"),
            }),
            Ok(content) => match AgentManifest::from_yaml(&content) {
                Err(e) => errors.push(ValidationError {
                    path: "manifest.yaml".into(),
                    message: format!("parse error: {e}"),
                }),
                Ok(m) => {
                    if m.metadata.name.is_empty() {
                        errors.push(ValidationError {
                            path: "manifest.yaml".into(),
                            message: "metadata.name is empty".into(),
                        });
                    }
                    if m.metadata.version.is_empty() {
                        errors.push(ValidationError {
                            path: "manifest.yaml".into(),
                            message: "metadata.version is empty".into(),
                        });
                    }
                    if let Some(ref prompt_path) = m.spec.system_prompt_path {
                        if !dir.join(prompt_path).exists() {
                            errors.push(ValidationError {
                                path: prompt_path.clone(),
                                message: "systemPromptPath references missing file".into(),
                            });
                        }
                    }
                }
            },
        }
    }

    #[instrument]
    fn validate_service(dir: &Path, errors: &mut Vec<ValidationError>) {
        let manifest_path = dir.join("manifest.yaml");
        match ServiceManifest::load(&manifest_path) {
            Err(e) => errors.push(ValidationError {
                path: "manifest.yaml".into(),
                message: format!("cannot read or parse: {e}"),
            }),
            Ok(m) => {
                if m.metadata.name.is_empty() {
                    errors.push(ValidationError {
                        path: "manifest.yaml".into(),
                        message: "metadata.name is empty".into(),
                    });
                }
                if m.metadata.version.is_empty() {
                    errors.push(ValidationError {
                        path: "manifest.yaml".into(),
                        message: "metadata.version is empty".into(),
                    });
                }
                if m.spec.binary.is_empty() {
                    errors.push(ValidationError {
                        path: "manifest.yaml".into(),
                        message: "spec.binary is empty".into(),
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

    const AGENT_MANIFEST: &str = r#"
apiVersion: avix/v1
kind: Agent
metadata:
  name: test
  version: "0.1.0"
spec:
  systemPromptPath: system-prompt.md
"#;

    const SERVICE_MANIFEST: &str = r#"
apiVersion: avix/v1
kind: Service
metadata:
  name: test-svc
  version: "0.1.0"
  description: Test service
spec:
  binary: /bin/test-svc
  tools:
    namespace: /tools/test/
"#;

    #[test]
    fn detect_agent_from_manifest_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), AGENT_MANIFEST).unwrap();

        let result = PackageType::detect(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Agent);
    }

    #[test]
    fn detect_service_from_manifest_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), SERVICE_MANIFEST).unwrap();

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
    fn detect_unknown_kind_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("manifest.yaml"),
            "apiVersion: avix/v1\nkind: Unknown\nmetadata:\n  name: x\n  version: 1.0.0\nspec: {}\n",
        )
        .unwrap();

        let result = PackageType::detect(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn valid_agent_pack_passes() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), AGENT_MANIFEST).unwrap();
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
            "apiVersion: avix/v1\nkind: Agent\nmetadata:\n  name: \"\"\n  version: \"0.1.0\"\nspec: {}\n",
        )
        .unwrap();

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
            "apiVersion: avix/v1\nkind: Agent\nmetadata:\n  name: test\n  version: \"0.1.0\"\nspec:\n  systemPromptPath: nonexistent.md\n",
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
        std::fs::write(dir.path().join("manifest.yaml"), SERVICE_MANIFEST).unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/test-svc"), "").unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PackageType::Service);
    }

    #[test]
    fn service_missing_manifest_errors() {
        let dir = TempDir::new().unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn service_missing_bin_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), SERVICE_MANIFEST).unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.path == "bin/"));
    }

    #[test]
    fn service_empty_bin_errors() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("manifest.yaml"), SERVICE_MANIFEST).unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();

        let result = PackageValidator::validate(dir.path());
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("empty")));
    }
}
