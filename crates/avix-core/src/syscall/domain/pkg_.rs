use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent_manifest::installer::{AgentInstallRequest, AgentInstaller, InstallScope};
use crate::error::AvixError;
use crate::packaging::trust::TrustStore;
use crate::service::installer::{InstallRequest, ServiceInstaller};
use crate::service::package_source::PackageSource;
use crate::syscall::{SyscallContext, SyscallError, SyscallResult};

pub struct InstallQuota {
    window: Duration,
    limit: u32,
    counters: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}

impl InstallQuota {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            window,
            limit,
            counters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn check(&self, username: &str) -> Result<(), SyscallError> {
        let mut map = self.counters.lock().unwrap();
        let now = Instant::now();
        let entry = map.entry(username.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) > self.window {
            *entry = (0, now);
        }
        if entry.0 >= self.limit {
            return Err(SyscallError::Eperm(
                0,
                format!(
                    "install quota exceeded: max {} installs per {:?}",
                    self.limit, self.window
                ),
            ));
        }
        entry.0 += 1;
        Ok(())
    }
}

lazy_static::lazy_static! {
    static ref INSTALL_QUOTA: InstallQuota = InstallQuota::new(10, Duration::from_secs(3600));
}

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
    let username = "default";
    INSTALL_QUOTA.check(username)?;

    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    
    let no_verify = params
        .get("no_verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    if !no_verify {
        check_untrusted_source(ctx, source)?;
    }

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
    let username = "default";
    INSTALL_QUOTA.check(username)?;

    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing source".into()))?;
    
    let no_verify = params
        .get("no_verify")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    
    if !no_verify {
        check_untrusted_source(ctx, source)?;
    }

    let version = params
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|v| *v != "latest")
        .map(|v| v.to_owned());
    let checksum = params
        .get("checksum")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
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

pub fn uninstall_agent(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "proc/package/install-agent")?;

    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;

    let username = "default";
    let scope = parse_scope(&params, username)?;
    let install_dir = match scope {
        InstallScope::System => avix_root.join("bin").join(name),
        InstallScope::User(u) => avix_root.join("users").join(u).join("bin").join(name),
    };

    if !install_dir.exists() {
        return Err(SyscallError::Einval(format!("agent not installed: {name}")));
    }

    std::fs::remove_dir_all(&install_dir)
        .map_err(|e| SyscallError::Eio(format!("failed to remove {}: {}", name, e)))?;

    Ok(json!({ "uninstalled": name }))
}

pub fn uninstall_service(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "proc/package/install-service")?;

    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SyscallError::Einval("missing name".into()))?;

    let install_dir = avix_root.join("services").join(name);

    if !install_dir.exists() {
        return Err(SyscallError::Einval(format!("service not installed: {name}")));
    }

    std::fs::remove_dir_all(&install_dir)
        .map_err(|e| SyscallError::Eio(format!("failed to remove {}: {}", name, e)))?;

    Ok(json!({ "uninstalled": name }))
}

pub fn trust_add(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "auth:admin")?;

    let key_asc = params["key_asc"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing key_asc".into()))?;
    let label = params["label"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing label".into()))?;
    let allowed_sources = params["allowed_sources"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect()
        })
        .unwrap_or_default();

    let trust_store = TrustStore::new(avix_root);
    let key = trust_store
        .add(key_asc, label, allowed_sources)
        .map_err(|e| SyscallError::Einval(e.to_string()))?;

    Ok(json!({
        "fingerprint": key.fingerprint,
        "label": key.label,
        "added_at": key.added_at.to_rfc3339(),
    }))
}

pub fn trust_list(_ctx: &SyscallContext, _params: Value, avix_root: &Path) -> SyscallResult {
    let trust_store = TrustStore::new(avix_root);
    let keys = trust_store
        .list()
        .map_err(|e| SyscallError::Eio(e.to_string()))?;
    let entries: Vec<_> = keys
        .iter()
        .map(|k| {
            json!({
                "fingerprint": k.fingerprint,
                "label": k.label,
                "added_at": k.added_at.to_rfc3339(),
                "allowed_sources": k.allowed_sources,
            })
        })
        .collect();
    Ok(json!({ "keys": entries }))
}

pub fn trust_remove(ctx: &SyscallContext, params: Value, avix_root: &Path) -> SyscallResult {
    check_capability(ctx, "auth:admin")?;
    let fingerprint = params["fingerprint"]
        .as_str()
        .ok_or_else(|| SyscallError::Einval("missing fingerprint".into()))?;
    let trust_store = TrustStore::new(avix_root);
    trust_store
        .remove(fingerprint)
        .map_err(|e| SyscallError::Einval(e.to_string()))?;
    Ok(json!({ "removed": fingerprint }))
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
