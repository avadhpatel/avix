use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{debug, info};

use crate::error::ClientError;
use crate::persistence::app_data_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Runtime root directory (where auth.conf, services/, proc/ live)
    pub root: PathBuf,
    /// Log level for the server
    pub log_level: String,
    /// Enable tracing output
    pub trace: bool,
    /// ATP server listen address
    pub address: String,
    /// ATP server port
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            root: {
                let mut d = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                d.push("avix-data");
                d
            },
            log_level: "warn".to_string(),
            trace: false,
            address: "0.0.0.0".to_string(),
            port: 9142,
        }
    }
}

impl ServerConfig {
    pub fn load() -> Result<Self, ClientError> {
        Self::load_from(None)
    }

    pub fn load_from(path: Option<PathBuf>) -> Result<Self, ClientError> {
        let path = path.unwrap_or_else(|| app_data_dir().join("server.yaml"));
        info!("Server config load {}", path.display());
        if !path.exists() {
            debug!("Server config not found, using defaults");
            return Ok(ServerConfig::default());
        }
        let content = std::fs::read_to_string(&path).map_err(|e| {
            ClientError::Other(anyhow::anyhow!("failed to read {}: {}", path.display(), e))
        })?;
        serde_yaml::from_str(&content).map_err(|e| {
            ClientError::Other(anyhow::anyhow!("failed to parse {}: {}", path.display(), e))
        })
    }

    pub fn save(&self) -> Result<(), ClientError> {
        let path = app_data_dir().join("server.yaml");
        info!("Server config save {}", path.display());
        let content = serde_yaml::to_string(self)
            .map_err(|e| ClientError::Other(anyhow::anyhow!("failed to serialize: {}", e)))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClientError::Other(anyhow::anyhow!("failed to create dir: {}", e)))?;
        }
        std::fs::write(&path, content).map_err(|e| {
            ClientError::Other(anyhow::anyhow!("failed to write {}: {}", path.display(), e))
        })
    }

    pub fn full_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn server_config_defaults() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.log_level, "warn");
        assert_eq!(cfg.port, 9142);
    }

    #[test]
    fn server_config_roundtrip() {
        let dir = TempDir::new().unwrap();
        let cfg = ServerConfig {
            root: PathBuf::from("/custom/root"),
            log_level: "debug".to_string(),
            trace: true,
            address: "127.0.0.1".to_string(),
            port: 9999,
        };
        let path = dir.path().join("server.yaml");
        let content = serde_yaml::to_string(&cfg).unwrap();
        std::fs::write(&path, &content).unwrap();
        let loaded = ServerConfig::load_from(Some(path)).unwrap();
        assert_eq!(loaded.log_level, "debug");
        assert_eq!(loaded.port, 9999);
    }

    #[test]
    fn full_url_constructs_correctly() {
        let cfg = ServerConfig {
            root: PathBuf::from("/tmp"),
            log_level: "info".to_string(),
            trace: false,
            address: "192.168.1.1".to_string(),
            port: 8080,
        };
        assert_eq!(cfg.full_url(), "http://192.168.1.1:8080");
    }
}
