use crate::error::AvixError;

use tracing::instrument;

#[derive(Debug, Clone)]
pub enum PackageSource {
    HttpUrl(String),
    LocalPath(std::path::PathBuf),
    GitHubRelease {
        url: String,
        checksum_url: Option<String>,
    },
    GitClone(String),
}

impl PackageSource {
    #[instrument]

    pub async fn resolve(source: &str, version: Option<&str>) -> Result<Self, AvixError> {
        if let Some(spec) = source.strip_prefix("github:") {
            return Self::resolve_github(spec, version).await;
        }
        if source.starts_with("github.com/") {
            return Self::resolve_github(source.trim_start_matches("github.com/"), version).await;
        }
        if let Some(repo_url) = source.strip_prefix("git:") {
            return Ok(Self::GitClone(repo_url.to_owned()));
        }
        if source.starts_with("https://") || source.starts_with("http://") {
            return Ok(Self::HttpUrl(source.to_owned()));
        }
        if let Some(path) = source.strip_prefix("file://") {
            return Ok(Self::LocalPath(std::path::PathBuf::from(path)));
        }
        if source.starts_with('/') || source.starts_with("./") || source.starts_with("../") {
            return Ok(Self::LocalPath(std::path::PathBuf::from(source)));
        }
        Err(AvixError::ConfigParse(format!(
            "unrecognized source: {source}"
        )))
    }

    #[instrument]

    async fn resolve_github(spec: &str, version: Option<&str>) -> Result<Self, AvixError> {
        let parts: Vec<&str> = spec.splitn(3, '/').collect();
        let (owner, repo, name) = match parts.as_slice() {
            [owner, repo, name] => (*owner, *repo, *name),
            [owner, name] => (*owner, "avix", *name),
            _ => {
                return Err(AvixError::ConfigParse(format!(
                    "invalid github: source '{spec}'"
                )))
            }
        };

        let tag = version.unwrap_or("latest");
        let api_url = if tag == "latest" {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}")
        };

        let client = reqwest::Client::builder()
            .user_agent("avix-installer/0.1")
            .build()
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

        let release: serde_json::Value = client
            .get(&api_url)
            .send()
            .await
            .map_err(|e| AvixError::ConfigParse(format!("GitHub API: {e}")))?
            .json()
            .await
            .map_err(|e| AvixError::ConfigParse(format!("GitHub API json: {e}")))?;

        let resolved_version = release["tag_name"]
            .as_str()
            .unwrap_or(tag)
            .trim_start_matches('v');
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        let candidates = [
            format!("{name}-v{resolved_version}-{os}-{arch}.tar.xz"),
            format!("{name}-v{resolved_version}.tar.xz"),
            format!("{name}-{resolved_version}.tar.xz"),
        ];

        let assets = release["assets"]
            .as_array()
            .ok_or_else(|| AvixError::ConfigParse("GitHub release has no assets".into()))?;

        let mut asset_url = None;
        let mut checksum_url = None;
        for candidate in &candidates {
            for asset in assets {
                let asset_name = asset["name"].as_str().unwrap_or("");
                if asset_name == candidate {
                    asset_url = Some(
                        asset["browser_download_url"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned(),
                    );
                }
                if asset_name == "checksums.sha256" {
                    checksum_url = Some(
                        asset["browser_download_url"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned(),
                    );
                }
            }
            if asset_url.is_some() {
                break;
            }
        }

        let url = asset_url.ok_or_else(|| {
            AvixError::ConfigParse(format!(
                "no matching asset for '{name}' in GitHub release {tag}"
            ))
        })?;

        Ok(Self::GitHubRelease { url, checksum_url })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_https_url() {
        let result = PackageSource::resolve("https://example.com/pkg.tar.xz", None).await;
        assert!(
            matches!(result, Ok(PackageSource::HttpUrl(url)) if url == "https://example.com/pkg.tar.xz")
        );
    }

    #[tokio::test]
    async fn resolve_local_abs_path() {
        let result = PackageSource::resolve("/abs/path/pkg.tar.xz", None).await;
        assert!(
            matches!(result, Ok(PackageSource::LocalPath(p)) if p == std::path::PathBuf::from("/abs/path/pkg.tar.xz"))
        );
    }

    #[tokio::test]
    async fn resolve_local_rel_path() {
        let result = PackageSource::resolve("./rel/path/pkg.tar.xz", None).await;
        assert!(
            matches!(result, Ok(PackageSource::LocalPath(p)) if p == std::path::PathBuf::from("./rel/path/pkg.tar.xz"))
        );
    }

    #[tokio::test]
    async fn resolve_file_scheme() {
        let result = PackageSource::resolve("file:///abs/path/pkg.tar.xz", None).await;
        assert!(
            matches!(result, Ok(PackageSource::LocalPath(p)) if p == std::path::PathBuf::from("/abs/path/pkg.tar.xz"))
        );
    }

    #[tokio::test]
    async fn resolve_git_clone() {
        let result = PackageSource::resolve("git:https://github.com/user/repo.git", None).await;
        assert!(
            matches!(result, Ok(PackageSource::GitClone(url)) if url == "https://github.com/user/repo.git")
        );
    }

    #[tokio::test]
    async fn resolve_github_two_part() {
        let result = PackageSource::resolve("github:owner/name", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_unknown_scheme_errors() {
        let result = PackageSource::resolve("ftp://example.com/pkg.tar.xz", None).await;
        assert!(result.is_err());
    }
}
