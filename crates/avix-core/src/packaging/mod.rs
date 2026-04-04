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

use std::path::Path;
use crate::error::AvixError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageType {
    Agent,
    Service,
}

impl PackageType {
    pub fn detect(dir: &Path) -> Result<Self, AvixError> {
        if dir.join("manifest.yaml").exists() {
            return Ok(Self::Agent);
        }
        if dir.join("service.yaml").exists() {
            return Ok(Self::Service);
        }
        Err(AvixError::ConfigParse(
            "cannot detect package type: no manifest.yaml or service.yaml found".into(),
        ))
    }
}
