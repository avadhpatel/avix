use serde_json::{json, Value};
use std::path::Path;

use crate::agent_manifest::installer::{AgentInstallRequest, AgentInstaller, InstallScope};
use crate::error::AvixError;
use crate::service::installer::{InstallRequest, ServiceInstaller};
use crate::service::package_source::PackageSource;
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

fn check_capability(ctx: &SyscallContext, cap: &str) -> Result<(), SyscallError> {
    if !ctx.token.has_tool(cap) {
        return Err(SyscallError::Eperm(
            ctx.caller_pid,
            format!("missing capability: {}", cap),
        ));
    }
    Ok(())
}

fn check_untrusted_source(ctx: &SyscallContext, source: &str) -> Result<(), SyscallError> {
    let is_official = source.contains("github.com/avadhpatel/avix")
        || source.starts_with("github:avadhpatel/avix");
    if !is_official {
        check_capability(ctx, "install:from-untrusted-source")?;
    }
    Ok(())
}

fn parse_scope(params: &Value, username: &str) -> Result<InstallScope, SyscallError> {
    match params
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("user")
    {
        "system" => Ok(InstallScope::System),
        _ => Ok(InstallScope::User(username.to_owned())),
    }
}

pub fn install_agent(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "proc/package/install-agent")?;

    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    check_untrusted_source(ctx, source)?;

    let username = "default";
    let scope = parse_scope(&params, username)?;
    let version = params
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|v| *v != "latest")
        .map(|v| v.to_owned());
    let checksum = params
        .get("checksum")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    let no_verify = params
        .get("no_verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let session_id = params
        .get("session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    let runtime = tokio::runtime::Handle::current();
    let result = runtime.block_on(async {
        let installer = AgentInstaller::new(avix_root.to_path_buf());
        installer
            .install(AgentInstallRequest {
                source: source.to_owned(),
                version,
                scope,
                checksum,
                session_id,
                no_verify,
            })
            .await
    });

    match result {
        Ok(r) => Ok(json!({
            "name": r.name,
            "version": r.version,
            "install_dir": r.install_dir.display().to_string()
        })),
        Err(e) => Err(SyscallError::Eio(e.to_string())),
    }
}

pub fn install_service(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "proc/package/install-service")?;

    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    check_untrusted_source(ctx, source)?;

    let version = params
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|v| *v != "latest")
        .map(|v| v.to_owned());
    let checksum = params
        .get("checksum")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    let no_verify = params
        .get("no_verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let _session_id = params
        .get("session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    let runtime = tokio::runtime::Handle::current();
    let result = runtime.block_on(async {
        let pkg_source = PackageSource::resolve(source, version.as_deref())
            .await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        let bytes = fetch_source_bytes(&pkg_source)
            .await
            .map_err(|e| AvixError::Io(e.to_string()))?;

        if !no_verify {
            if let Some(ck) = &checksum {
                ServiceInstaller::static_verify_checksum(&bytes, ck)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            }
        }

        let req = InstallRequest {
            source: source.to_owned(),
            checksum,
            autostart: false,
        };
        let installer = ServiceInstaller::new(avix_root.to_path_buf());
        installer.install(req).await
    });

    match result {
        Ok(r) => Ok(json!({
            "name": r.name,
            "version": r.version,
            "install_dir": r.install_dir.display().to_string()
        })),
        Err(e) => Err(SyscallError::Eio(e.to_string())),
    }
}

async fn fetch_source_bytes(source: &PackageSource) -> Result<Vec<u8>, AvixError> {
    match source {
        PackageSource::HttpUrl(url) => {
            let bytes = reqwest::get(url)
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch {}: {}", url, e)))?
                .bytes()
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch body {}: {}", url, e)))?;
            Ok(bytes.to_vec())
        }
        PackageSource::LocalPath(path) => std::fs::read(path)
            .map_err(|e| AvixError::ConfigParse(format!("read {}: {}", path.display(), e))),
        PackageSource::GitHubRelease { url, .. } => {
            let bytes = reqwest::get(url)
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch {}: {}", url, e)))?
                .bytes()
                .await
                .map_err(|e| AvixError::ConfigParse(format!("fetch body {}: {}", url, e)))?;
            Ok(bytes.to_vec())
        }
        PackageSource::GitClone(_) => {
            Err(AvixError::ConfigParse("git clone not implemented".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::token::CapabilityToken;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_ctx(tools: &[&str]) -> SyscallContext {
        SyscallContext {
            caller_pid: 42,
            token: CapabilityToken::test_token(tools),
        }
    }

    #[test]
    fn missing_install_capability_eperm() {
        let ctx = make_ctx(&[]);
        let avix_root = PathBuf::from("/tmp");
        let result = install_agent(
            &ctx,
            json!({"source": "file:///tmp/test"}),
            avix_root.as_path(),
        );
        assert!(matches!(result, Err(SyscallError::Eperm(_, _))));
    }

    #[test]
    fn untrusted_source_without_cap_eperm() {
        let ctx = make_ctx(&["proc/package/install-agent"]);
        let avix_root = PathBuf::from("/tmp");
        let result = install_agent(
            &ctx,
            json!({"source": "https://example.com/agent.tar.xz"}),
            avix_root.as_path(),
        );
        assert!(matches!(result, Err(SyscallError::Eperm(_, _))));
    }

    #[test]
    #[ignore = "requires network access to GitHub API"]
    fn official_source_no_untrusted_cap_ok() {
        let ctx = make_ctx(&["proc/package/install-agent"]);
        let avix_root = PathBuf::from("/tmp");
        let result = install_agent(
            &ctx,
            json!({"source": "github:avadhpatel/avix/test-agent"}),
            avix_root.as_path(),
        );
        assert!(matches!(result, Err(SyscallError::Eperm(_, _))));
    }

    #[tokio::test]
    #[ignore = "requires tokio runtime in test context"]
    async fn install_agent_local_path() {
        use xz2::write::XzEncoder;

        let dir = TempDir::new().unwrap();
        let mut buf = Vec::new();
        {
            let enc = XzEncoder::new(&mut buf, 6);
            let mut ar = tar::Builder::new(enc);
            let manifest = "name: test-agent\nversion: 1.0.0\ndescription: test\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(
                &mut header,
                "test-agent-1.0.0/manifest.yaml",
                manifest.as_bytes(),
            )
            .unwrap();
            ar.finish().unwrap();
        }
        let pkg_path = dir.path().join("agent.tar.xz");
        std::fs::write(&pkg_path, &buf).unwrap();

        let root = TempDir::new().unwrap();
        let ctx = make_ctx(&["proc/package/install-agent"]);
        let result = install_agent(
            &ctx,
            json!({"source": format!("file://{}", pkg_path.display()), "scope": "user"}),
            root.path(),
        );
        assert!(result.is_ok());
    }
}
