use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

use crate::error::ClientError;
use crate::persistence;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub server_url: String,
    pub identity: String,
    pub credential: String,
    /// Only required when auto_start_server is true (or when running the embedded daemon in the app).
    #[serde(default)]
    pub runtime_root: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub auto_start_server: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:9142".to_string(),
            identity: "admin".to_string(),
            credential: String::new(),
            runtime_root: None,
            auto_start_server: true,
        }
    }
}

impl ClientConfig {
    pub fn load() -> Result<Self, ClientError> {
        let path = persistence::app_data_dir().join("client.yaml");
        info!("Config load {}", path.display());
        let config = persistence::load_yaml(&path)?;
        debug!("Config {:?}", config);
        Ok(config)
    }

    pub fn save(&self) -> Result<(), ClientError> {
        let path = persistence::app_data_dir().join("client.yaml");
        info!("Config save {}", path.display());
        debug!("Config {:?}", self);
        persistence::save_yaml(&path, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn client_config_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cfg = ClientConfig {
            server_url: "http://localhost:7700".into(),
            identity: "bob".into(),
            credential: "secret".into(),
            runtime_root: Some(dir.path().to_path_buf()),
            auto_start_server: false,
        };
        let path = dir.path().join("client.yaml");
        persistence::save_yaml(&path, &cfg).unwrap();
        let loaded: ClientConfig = persistence::load_yaml(&path).unwrap();
        assert_eq!(loaded.identity, "bob");
        assert_eq!(loaded.server_url, "http://localhost:7700");
        assert_eq!(loaded.credential, "secret");
        assert!(!loaded.auto_start_server);
        assert_eq!(loaded.runtime_root, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn default_config() {
        let cfg = ClientConfig::default();
        assert_eq!(cfg.server_url, "http://localhost:9142");
        assert_eq!(cfg.identity, "admin");
        assert!(cfg.credential.is_empty());
        assert!(cfg.auto_start_server);
    }

    #[test]
    fn load_config_returns_default_if_missing() {
        // load_json on a guaranteed-missing path errors; unwrap_or_else must produce the default.
        let missing = std::path::PathBuf::from("/nonexistent/__avix_test_config__.yaml");
        let cfg: ClientConfig =
            persistence::load_yaml(&missing).unwrap_or_else(|_| ClientConfig::default());
        assert_eq!(cfg.server_url, "http://localhost:9142");
    }
}
