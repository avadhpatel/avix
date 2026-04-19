use serde::{Deserialize, Serialize};

use tracing::instrument;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallReceipt {
    pub name: String,
    pub version: String,
    pub source_url: Option<String>,
    pub checksum: Option<String>, // "sha256:abc123..."
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub service_unit_path: String,
    pub binary_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_receipt_roundtrip() {
        let receipt = InstallReceipt {
            name: "github-svc".to_string(),
            version: "1.0.0".to_string(),
            source_url: Some("https://example.com/github-svc.tar.gz".to_string()),
            checksum: Some("sha256:abc123".to_string()),
            installed_at: chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .into(),
            service_unit_path: "/services/github-svc/service.yaml".to_string(),
            binary_path: "/services/github-svc/bin/github-svc".to_string(),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let decoded: InstallReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, receipt.name);
        assert_eq!(decoded.version, receipt.version);
        assert_eq!(decoded.checksum, receipt.checksum);
        assert_eq!(decoded.binary_path, receipt.binary_path);
    }

    #[test]
    fn install_receipt_optional_fields_none() {
        let receipt = InstallReceipt {
            name: "min-svc".to_string(),
            version: "0.1.0".to_string(),
            source_url: None,
            checksum: None,
            installed_at: chrono::Utc::now(),
            service_unit_path: "/services/min-svc/service.yaml".to_string(),
            binary_path: "/services/min-svc/bin/min-svc".to_string(),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        let decoded: InstallReceipt = serde_json::from_str(&json).unwrap();
        assert!(decoded.source_url.is_none());
        assert!(decoded.checksum.is_none());
    }
}
