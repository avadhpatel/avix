use crate::error::AvixError;
use std::path::PathBuf;

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

    Ok(ConfigInitResult { api_key: raw_key })
}
