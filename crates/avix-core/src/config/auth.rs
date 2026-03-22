use crate::error::AvixError;
use crate::types::Role;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialType {
    ApiKey {
        key_hash: String,
        #[serde(default)]
        header: Option<String>,
    },
    Password {
        password_hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthIdentity {
    pub name: String,
    pub uid: u32,
    pub role: Role,
    pub credential: CredentialType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthPolicy {
    pub session_ttl: String,
    #[serde(default)]
    pub require_tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub policy: AuthPolicy,
    pub identities: Vec<AuthIdentity>,
}

impl AuthConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self =
            serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), AvixError> {
        if self.identities.is_empty() {
            return Err(AvixError::ConfigParse(
                "identities must not be empty".into(),
            ));
        }
        let has_admin = self.identities.iter().any(|i| i.role == Role::Admin);
        if !has_admin {
            return Err(AvixError::ConfigParse(
                "at least one identity must have role: admin".into(),
            ));
        }
        Ok(())
    }
}
