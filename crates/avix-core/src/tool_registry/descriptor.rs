use serde::{Deserialize, Serialize};

use crate::tool_registry::permissions::ToolPermissions;

/// Typed tool descriptor, parsed from `<name>.tool.yaml`.
/// Matches the format defined in docs/architecture/07-services.md § Tool Descriptor Format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
    /// Legacy single-field owner. When `permissions` is absent, this seeds the owner name.
    #[serde(default)]
    pub owner: Option<String>,
    /// Full permission block. When present, takes precedence over the bare `owner` field.
    #[serde(default)]
    pub permissions: Option<ToolPermissions>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: ToolDescriptorStatus,
    #[serde(default)]
    pub ipc: Option<IpcBinding>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub job: bool,
    #[serde(default)]
    pub job_timeout: Option<String>,
    #[serde(default)]
    pub capabilities_required: Vec<String>,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub visibility: ToolVisibilitySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDescriptorStatus {
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Default for ToolDescriptorStatus {
    fn default() -> Self {
        Self {
            state: default_state(),
            reason: None,
        }
    }
}

fn default_state() -> String {
    "available".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcBinding {
    #[serde(default = "default_transport")]
    pub transport: String,
    pub endpoint: String,
    pub method: String,
}

fn default_transport() -> String {
    "local-ipc".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolVisibilitySpec {
    #[default]
    All,
    User(String),
    Crew(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_descriptor_parses() {
        let desc: ToolDescriptor =
            serde_yaml::from_str("name: fs/read\ndescription: Read a file\n").unwrap();
        assert_eq!(desc.name, "fs/read");
        assert_eq!(desc.description, "Read a file");
        assert!(!desc.streaming);
        assert!(!desc.job);
        assert_eq!(desc.status.state, "available");
        assert_eq!(desc.visibility, ToolVisibilitySpec::All);
    }

    #[test]
    fn job_flag_and_timeout_parse() {
        let desc: ToolDescriptor = serde_yaml::from_str(
            "name: video/transcode\ndescription: Encode\njob: true\njob_timeout: 3600s\n",
        )
        .unwrap();
        assert!(desc.job);
        assert_eq!(desc.job_timeout.as_deref(), Some("3600s"));
    }

    #[test]
    fn ipc_binding_defaults_transport() {
        let desc: ToolDescriptor = serde_yaml::from_str(
            "name: x/y\ndescription: d\nipc:\n  endpoint: memfs\n  method: fs.read\n",
        )
        .unwrap();
        let ipc = desc.ipc.unwrap();
        assert_eq!(ipc.transport, "local-ipc");
        assert_eq!(ipc.endpoint, "memfs");
        assert_eq!(ipc.method, "fs.read");
    }

    #[test]
    fn permissions_block_parsed() {
        let yaml = "name: fs/read\ndescription: d\npermissions:\n  owner: alice\n  crew: ops\n  all: \"r--\"\n";
        let desc: ToolDescriptor = serde_yaml::from_str(yaml).unwrap();
        let perms = desc.permissions.unwrap();
        assert_eq!(perms.owner, "alice");
        assert_eq!(perms.crew, "ops");
        assert_eq!(perms.all, "r--");
    }

    #[test]
    fn permissions_absent_yields_none() {
        let desc: ToolDescriptor =
            serde_yaml::from_str("name: fs/read\ndescription: d\n").unwrap();
        assert!(desc.permissions.is_none());
    }

    #[test]
    fn owner_field_without_permissions_block() {
        let desc: ToolDescriptor =
            serde_yaml::from_str("name: fs/read\ndescription: d\nowner: bob\n").unwrap();
        assert_eq!(desc.owner.as_deref(), Some("bob"));
        assert!(desc.permissions.is_none());
    }
}
