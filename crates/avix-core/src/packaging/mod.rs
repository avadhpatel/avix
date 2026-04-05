pub mod builder;
pub mod gpg;
pub mod scaffold;
pub mod trust;
pub mod validator;

pub use builder::{BuildRequest, BuildResult, PackageBuilder};
pub use gpg::{verify_signature, VerifiedBy};
pub use scaffold::{PackageScaffold, ScaffoldRequest};
pub use trust::{TrustStore, TrustedKey};
pub use validator::{PackageValidator, ValidationError};

use crate::error::AvixError;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageType {
    Agent,
    Service,
}

impl PackageType {
    pub fn detect(dir: &Path) -> Result<Self, AvixError> {
        let path = dir.join("manifest.yaml");
        let content = std::fs::read_to_string(&path)
            .map_err(|_| AvixError::ConfigParse("manifest.yaml not found".into()))?;
        #[derive(serde::Deserialize)]
        struct KindProbe {
            kind: String,
        }
        let probe: KindProbe = serde_yaml::from_str(&content)
            .map_err(|e| AvixError::ConfigParse(format!("manifest.yaml parse error: {e}")))?;
        match probe.kind.as_str() {
            "Agent" => Ok(Self::Agent),
            "Service" => Ok(Self::Service),
            other => Err(AvixError::ConfigParse(format!("unknown kind: {other}"))),
        }
    }
}
