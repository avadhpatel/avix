use crate::error::AvixError;
use std::path::{Path, PathBuf};

pub struct ConfigInitParams {
    pub root: PathBuf,
    pub identity_name: String,
    pub credential_type: String,
    pub role: String,
    pub master_key_source: String,
    pub mode: String,
}

pub struct ConfigInitResult {
    pub api_key: String,
}

/// Write `content` to `path` only if the file does not already exist.
/// Returns `true` if the file was written, `false` if it was skipped.
fn write_if_absent(path: &Path, content: &str) -> Result<bool, AvixError> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    }
    std::fs::write(path, content).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    Ok(true)
}

pub fn run_config_init(params: ConfigInitParams) -> Result<ConfigInitResult, AvixError> {
    let etc_dir = params.root.join("etc");

    // Idempotent: if auth.conf exists, return ok
    if etc_dir.join("auth.conf").exists() {
        return Ok(ConfigInitResult {
            api_key: "sk-avix-existing".into(),
        });
    }

    std::fs::create_dir_all(&etc_dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    let raw_key = format!("sk-avix-{}", uuid::Uuid::new_v4());

    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(b"config-init-secret").expect("HMAC accepts any key size");
    mac.update(raw_key.as_bytes());
    let key_hash = format!("hmac-sha256:{}", hex::encode(mac.finalize().into_bytes()));

    let auth_conf = format!(
        r#"apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
  require_tls: false
identities:
  - name: {name}
    uid: 1001
    role: {role}
    credential:
      type: api_key
      key_hash: "{key_hash}"
"#,
        name = params.identity_name,
        role = params.role,
        key_hash = key_hash,
    );

    std::fs::write(etc_dir.join("auth.conf"), auth_conf)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    // ── Additional /etc/avix/ config files ───────────────────────────────────

    let root = &params.root;
    let now = chrono::Utc::now().to_rfc3339();
    let identity = &params.identity_name;
    let role = &params.role;
    let cred_type = &params.credential_type;
    let root_str = root.display();

    write_if_absent(
        &root.join("etc/kernel.yaml"),
        &KERNEL_YAML_TEMPLATE.replace("{now}", &now),
    )?;

    write_if_absent(
        &root.join("etc/users.yaml"),
        &USERS_YAML_TEMPLATE
            .replace("{identity}", identity)
            .replace("{role}", role)
            .replace("{cred_type}", cred_type)
            .replace("{key_hash}", &key_hash),
    )?;

    write_if_absent(&root.join("etc/crews.yaml"), CREWS_YAML_TEMPLATE)?;

    write_if_absent(&root.join("etc/crontab.yaml"), CRONTAB_YAML_TEMPLATE)?;

    let root_s = root_str.to_string();
    write_if_absent(
        &root.join("etc/fstab.yaml"),
        &FSTAB_YAML_TEMPLATE
            .replace("{root}", &root_s)
            .replace("{identity}", identity),
    )?;

    // Data directories referenced by fstab mounts
    std::fs::create_dir_all(root.join(format!("data/users/{identity}")))
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    std::fs::create_dir_all(root.join("secrets"))
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    Ok(ConfigInitResult { api_key: raw_key })
}

// ── YAML templates ────────────────────────────────────────────────────────────

const KERNEL_YAML_TEMPLATE: &str = "apiVersion: avix/v1
kind: KernelConfig
metadata:
  createdAt: \"{now}\"
spec:
  log:
    level: info
    format: json
  secrets:
    algorithm: aes-256-gcm
    masterKey:
      source: env
      envVar: AVIX_MASTER_KEY
    store:
      path: /secrets
      provider: local
    audit:
      enabled: true
      logReads: true
      logWrites: true
";

const USERS_YAML_TEMPLATE: &str = "apiVersion: avix/v1
kind: Users
spec:
  users:
    - name: \"{identity}\"
      uid: 1001
      role: \"{role}\"
      credential:
        type: \"{cred_type}\"
        keyHash: \"{key_hash}\"
";

const CREWS_YAML_TEMPLATE: &str = "apiVersion: avix/v1
kind: Crews
spec:
  crews: []
";

const CRONTAB_YAML_TEMPLATE: &str = "apiVersion: avix/v1
kind: Crontab
spec:
  jobs: []
";

const FSTAB_YAML_TEMPLATE: &str = "apiVersion: avix/v1
kind: Fstab
spec:
  mounts:
    - path: /etc/avix
      provider: local
      config:
        root: \"{root}/etc\"
      options:
        readonly: false

    - path: /users/{identity}
      provider: local
      config:
        root: \"{root}/data/users/{identity}\"
      options: {{}}

    - path: /secrets
      provider: local
      config:
        root: \"{root}/secrets\"
      options:
        encrypted: true
";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_init_creates_auth_conf() {
        let dir = tempdir().unwrap();
        let params = ConfigInitParams {
            root: dir.path().to_path_buf(),
            identity_name: "alice".into(),
            credential_type: "api_key".into(),
            role: "admin".into(),
            master_key_source: "env".into(),
            mode: "cli".into(),
        };
        let result = run_config_init(params).unwrap();
        assert!(result.api_key.starts_with("sk-avix-"));
        // auth.conf should exist
        assert!(dir.path().join("etc/auth.conf").exists());
    }

    #[test]
    fn test_config_init_idempotent() {
        let dir = tempdir().unwrap();

        // First init
        let params1 = ConfigInitParams {
            root: dir.path().to_path_buf(),
            identity_name: "alice".into(),
            credential_type: "api_key".into(),
            role: "admin".into(),
            master_key_source: "env".into(),
            mode: "cli".into(),
        };
        run_config_init(params1).unwrap();

        // Second init — should return existing key
        let params2 = ConfigInitParams {
            root: dir.path().to_path_buf(),
            identity_name: "bob".into(),
            credential_type: "api_key".into(),
            role: "admin".into(),
            master_key_source: "env".into(),
            mode: "cli".into(),
        };
        let result2 = run_config_init(params2).unwrap();
        // Returns fixed "existing" key
        assert_eq!(result2.api_key, "sk-avix-existing");
    }

    #[test]
    fn test_config_init_auth_conf_content() {
        let dir = tempdir().unwrap();
        let params = ConfigInitParams {
            root: dir.path().to_path_buf(),
            identity_name: "testuser".into(),
            credential_type: "api_key".into(),
            role: "operator".into(),
            master_key_source: "env".into(),
            mode: "cli".into(),
        };
        run_config_init(params).unwrap();

        let auth_conf = std::fs::read_to_string(dir.path().join("etc/auth.conf")).unwrap();
        assert!(auth_conf.contains("testuser"));
        assert!(auth_conf.contains("operator"));
        assert!(auth_conf.contains("api_key"));
        assert!(auth_conf.contains("hmac-sha256:"));
    }

    #[test]
    fn test_config_init_api_key_format() {
        let dir = tempdir().unwrap();
        let params = ConfigInitParams {
            root: dir.path().to_path_buf(),
            identity_name: "carol".into(),
            credential_type: "api_key".into(),
            role: "viewer".into(),
            master_key_source: "env".into(),
            mode: "cli".into(),
        };
        let result = run_config_init(params).unwrap();
        // Key should be sk-avix-<uuid>
        assert!(result.api_key.starts_with("sk-avix-"));
        // UUID portion should be 36 chars
        let uuid_part = &result.api_key["sk-avix-".len()..];
        assert_eq!(uuid_part.len(), 36);
    }
}
