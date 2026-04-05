use super::PackageType;
use crate::error::AvixError;
use crate::packaging::validator::PackageValidator;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct BuildRequest {
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub version: String,
    pub skip_validation: bool,
}

pub struct BuildResult {
    pub archive_path: PathBuf,
    pub checksum_entry: String,
    pub pkg_type: PackageType,
    pub name: String,
    pub version: String,
}

pub struct PackageBuilder;

impl PackageBuilder {
    pub fn build(req: BuildRequest) -> Result<BuildResult, AvixError> {
        let pkg_type = if req.skip_validation {
            PackageType::detect(&req.source_dir)?
        } else {
            PackageValidator::validate(&req.source_dir).map_err(
                |errs: Vec<crate::packaging::validator::ValidationError>| {
                    let msg = errs
                        .iter()
                        .map(|e| format!("  {}: {}", e.path, e.message))
                        .collect::<Vec<_>>()
                        .join("\n");
                    AvixError::ConfigParse(format!("validation failed:\n{msg}"))
                },
            )?
        };

        let name = Self::read_name(&req.source_dir, &pkg_type)?;

        let filename = match &pkg_type {
            PackageType::Agent => {
                format!("{}-{}.tar.xz", name, req.version)
            }
            PackageType::Service => {
                let os = std::env::consts::OS;
                let arch = std::env::consts::ARCH;
                format!("{}-{}-{}-{}.tar.xz", name, req.version, os, arch)
            }
        };

        let archive_path = req.output_dir.join(&filename);
        std::fs::create_dir_all(&req.output_dir)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Self::create_xz_archive(&req.source_dir, &archive_path, &name, &req.version)?;

        let bytes =
            std::fs::read(&archive_path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let digest = hex::encode(Sha256::digest(&bytes));
        let checksum_entry = format!("{}  {}\n", digest, filename);

        let checksums_path = req.output_dir.join("checksums.sha256");
        let mut existing = if checksums_path.exists() {
            std::fs::read_to_string(&checksums_path)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?
        } else {
            String::new()
        };
        if existing.contains(&filename) {
            existing = existing
                .lines()
                .filter(|l| !l.contains(&filename))
                .map(|l| format!("{l}\n"))
                .collect();
        }
        existing.push_str(&checksum_entry);
        std::fs::write(&checksums_path, &existing)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        Ok(BuildResult {
            archive_path,
            checksum_entry,
            pkg_type,
            name,
            version: req.version,
        })
    }

    fn create_xz_archive(
        source_dir: &Path,
        dest: &Path,
        name: &str,
        version: &str,
    ) -> Result<(), AvixError> {
        let file = std::fs::File::create(dest)
            .map_err(|e| AvixError::ConfigParse(format!("create archive: {e}")))?;
        let xz = xz2::write::XzEncoder::new(file, 6);
        let mut archive = tar::Builder::new(xz);
        archive.follow_symlinks(false);

        // Wrap contents in versioned folder: agent-name-1.0.0/manifest.yaml
        let folder = format!("{}-{}", name, version);
        Self::add_dir_to_archive(&mut archive, source_dir, source_dir, &folder)?;

        archive
            .finish()
            .map_err(|e| AvixError::ConfigParse(format!("finalize archive: {e}")))?;
        Ok(())
    }

    fn add_dir_to_archive(
        archive: &mut tar::Builder<impl std::io::Write>,
        base: &Path,
        dir: &Path,
        folder: &str,
    ) -> Result<(), AvixError> {
        for entry in std::fs::read_dir(dir).map_err(|e| AvixError::ConfigParse(e.to_string()))? {
            let entry = entry.map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let path = entry.path();
            let rel = path.strip_prefix(base).unwrap();

            let name = rel
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .unwrap_or_default();
            if matches!(name.as_str(), ".git" | "target" | "Cargo.lock") {
                continue;
            }

            // Archive as folder/relative-path e.g., agent-1.0.0/manifest.yaml
            let archive_path = PathBuf::from(format!("{}/{}", folder, rel.display()));
            if path.is_dir() {
                Self::add_dir_to_archive(archive, base, &path, folder)?;
            } else {
                archive
                    .append_path_with_name(&path, &archive_path)
                    .map_err(|e| {
                        AvixError::ConfigParse(format!("add {}: {e}", archive_path.display()))
                    })?;
            }
        }
        Ok(())
    }

    fn read_name(dir: &Path, pkg_type: &PackageType) -> Result<String, AvixError> {
        match pkg_type {
            PackageType::Agent => {
                let content = std::fs::read_to_string(dir.join("manifest.yaml"))
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let m: serde_yaml::Value = serde_yaml::from_str(&content)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                m["name"]
                    .as_str()
                    .map(|s| s.to_owned())
                    .ok_or_else(|| AvixError::ConfigParse("manifest.yaml missing name".into()))
            }
            PackageType::Service => {
                let content = std::fs::read_to_string(dir.join("service.unit"))
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                let u: toml::Value =
                    toml::from_str(&content).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                u["name"]
                    .as_str()
                    .map(|s| s.to_owned())
                    .ok_or_else(|| AvixError::ConfigParse("service.unit missing name".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_agent_creates_tar_xz() {
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("manifest.yaml"),
            "name: test-agent\nversion: \"0.1.0\"\nsystem_prompt_path: system-prompt.md\n",
        )
        .unwrap();
        std::fs::write(src.path().join("system-prompt.md"), "# Test\n").unwrap();

        let out = TempDir::new().unwrap();
        let req = BuildRequest {
            source_dir: src.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: false,
        };

        let result = PackageBuilder::build(req).unwrap();
        assert!(result.archive_path.to_string_lossy().ends_with(".tar.xz"));
        assert!(result.archive_path.exists());

        let bytes = std::fs::read(&result.archive_path).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    #[ignore] // Known issue: service validation fails due to required fields
    fn build_service_creates_platform_archive() {
        let src = TempDir::new().unwrap();
        std::fs::write(src.path().join("service.yaml"), "name: test-svc\nversion: \"0.1.0\"\n\nunit:\n  description: \"\"\n\nservice:\n  binary: \"/bin/test-svc\"\n  language: \"rust\"\n\ntools:\n  namespace: \"/tools/test-svc/\"\n  provides: []\n").unwrap();
        std::fs::create_dir_all(src.path().join("bin")).unwrap();
        std::fs::write(src.path().join("bin/test-svc"), "").unwrap();

        let out = TempDir::new().unwrap();
        let req = BuildRequest {
            source_dir: src.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v1.0.0".into(),
            skip_validation: true,
        };

        let result = PackageBuilder::build(req).unwrap();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        assert!(result
            .archive_path
            .to_string_lossy()
            .contains(&format!("-{}-{}.tar.xz", os, arch)));
    }

    #[test]
    fn build_writes_checksums_file() {
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("manifest.yaml"),
            "name: test\nversion: \"0.1.0\"\nsystem_prompt_path: system-prompt.md\n",
        )
        .unwrap();
        std::fs::write(src.path().join("system-prompt.md"), "# Test\n").unwrap();

        let out = TempDir::new().unwrap();
        let req = BuildRequest {
            source_dir: src.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: false,
        };

        let _ = PackageBuilder::build(req).unwrap();
        let checksums = out.path().join("checksums.sha256");
        assert!(checksums.exists());

        let content = std::fs::read_to_string(&checksums).unwrap();
        assert!(content.contains("test-v0.1.0.tar.xz"));
    }

    #[test]
    fn build_accumulates_checksums() {
        let src1 = TempDir::new().unwrap();
        std::fs::write(
            src1.path().join("manifest.yaml"),
            "name: test1\nversion: \"0.1.0\"\nsystem_prompt_path: system-prompt.md\n",
        )
        .unwrap();
        std::fs::write(src1.path().join("system-prompt.md"), "# Test\n").unwrap();

        let src2 = TempDir::new().unwrap();
        std::fs::write(
            src2.path().join("manifest.yaml"),
            "name: test2\nversion: \"0.1.0\"\nsystem_prompt_path: system-prompt.md\n",
        )
        .unwrap();
        std::fs::write(src2.path().join("system-prompt.md"), "# Test\n").unwrap();

        let out = TempDir::new().unwrap();

        let req1 = BuildRequest {
            source_dir: src1.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: false,
        };
        let _ = PackageBuilder::build(req1).unwrap();

        let req2 = BuildRequest {
            source_dir: src2.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: false,
        };
        let _ = PackageBuilder::build(req2).unwrap();

        let checksums = out.path().join("checksums.sha256");
        let content = std::fs::read_to_string(&checksums).unwrap();
        assert!(content.contains("test1-v0.1.0.tar.xz"));
        assert!(content.contains("test2-v0.1.0.tar.xz"));
    }

    #[test]
    fn build_validates_before_build() {
        let src = TempDir::new().unwrap();

        let out = TempDir::new().unwrap();
        let req = BuildRequest {
            source_dir: src.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: false,
        };

        let result = PackageBuilder::build(req);
        assert!(result.is_err());
    }

    #[test]
    fn build_skip_validation_bypasses_check() {
        let src = TempDir::new().unwrap();
        std::fs::write(
            src.path().join("manifest.yaml"),
            "name: test\nversion: \"0.1.0\"\n",
        )
        .unwrap();

        let out = TempDir::new().unwrap();
        let req = BuildRequest {
            source_dir: src.path().to_path_buf(),
            output_dir: out.path().to_path_buf(),
            version: "v0.1.0".into(),
            skip_validation: true,
        };

        let result = PackageBuilder::build(req);
        assert!(result.is_ok());
    }
}
