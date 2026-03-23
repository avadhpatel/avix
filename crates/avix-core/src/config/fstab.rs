use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parsed representation of `etc/fstab.yaml`.
///
/// Used in Phase 2 bootstrap to determine which VFS paths are backed by disk.
/// In v0.1 the four invariant mounts are hard-coded in `phase2::mount_persistent_trees`;
/// this struct is a stub for future use when the full mount system lands in v0.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FstabConfig {
    pub api_version: String,
    pub kind: String,
    pub spec: FstabSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabSpec {
    pub mounts: Vec<FstabMount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FstabMount {
    pub path: String,
    pub provider: String,
    #[serde(default)]
    pub config: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub options: HashMap<String, serde_yaml::Value>,
}

impl FstabConfig {
    pub fn parse(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }
}
