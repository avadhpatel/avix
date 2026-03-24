use serde::{Deserialize, Serialize};
use std::env;
use std::path::PathBuf;
use dirs;

use crate::error::ClientError;
use crate::persistence;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub server_url: String,
    pub identity: String,
    pub credential: String,
    pub runtime_root: PathBuf,
    #[serde(default = "default_true")]
    pub auto_start_server: bool,
}

fn default_true() -> bool { true }

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:7700".to_string(),
            identity: "admin".to_string(),
            credential: String::new(),
            runtime_root: dirs::home_dir()
                .unwrap_or_else(|| {
                    env::var("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("./"))
                })
                .join("avix-data"),
            auto_start_server: true,
        }
    }
}

impl ClientConfig {
    pub fn load() -> Result<Self, ClientError> {
        persistence::load_json(&persistence::app_data_dir().join("client.json"))
    }

    pub fn save(&self) -> Result<(), ClientError> {
        persistence::save_json(&persistence::app_data_dir().join("client.json"), self)
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
            runtime_root: dir.path().to_path_buf(),
            auto_start_server: false,
        };
        let path = dir.path().join("client.json");
        persistence::save_json(&path, &cfg).unwrap();
        let loaded: ClientConfig = persistence::load_json(&path).unwrap();
        assert_eq!(loaded.identity, "bob");
        assert_eq!(loaded.server_url, "http://localhost:7700");
        assert_eq!(loaded.credential, "secret");
        assert!(!loaded.auto_start_server);
        assert_eq!(loaded.runtime_root, dir.path().to_path_buf());
    }

    #[test]
    fn default_config() {
        let cfg = ClientConfig::default();
        assert_eq!(cfg.server_url, "http://127.0.0.1:7700");
        assert_eq!(cfg.identity, "admin");
        assert!(cfg.credential.is_empty());
        assert!(cfg.auto_start_server);
    }
}