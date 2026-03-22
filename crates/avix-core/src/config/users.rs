use crate::error::AvixError;
use crate::types::Role;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserQuota {
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(rename = "requestsPerDay", default)]
    pub requests_per_day: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub role: Role,
    #[serde(rename = "additionalTools", default)]
    pub additional_tools: Vec<String>,
    #[serde(rename = "deniedTools", default)]
    pub denied_tools: Vec<String>,
    #[serde(default)]
    pub quota: Option<UserQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsersConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub users: Vec<User>,
}

impl UsersConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self =
            serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), AvixError> {
        let mut seen_uids = std::collections::HashSet::new();
        for user in &self.users {
            if !seen_uids.insert(user.uid) {
                return Err(AvixError::ConfigParse(format!(
                    "duplicate uid: {}",
                    user.uid
                )));
            }
        }
        Ok(())
    }
}
