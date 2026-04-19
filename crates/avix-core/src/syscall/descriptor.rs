use serde::{Deserialize, Serialize};
use tracing::instrument;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyscallDescriptor {
    pub name: String,
    pub description: String,
    pub short: String,
    pub detailed: String,
    pub domain: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub capabilities_required: Vec<String>,
    pub handler_signature: String,
}

impl SyscallDescriptor {
    #[instrument]
    pub fn new(
        name: &str,
        domain: &str,
        short: &str,
        detailed: &str,
        capabilities: Vec<&str>,
        handler_sig: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: short.to_string(),
            short: short.to_string(),
            detailed: detailed.to_string(),
            domain: domain.to_string(),
            input_schema: serde_json::Value::Null,
            output_schema: serde_json::Value::Null,
            capabilities_required: capabilities.iter().map(|s| s.to_string()).collect(),
            handler_signature: handler_sig.to_string(),
        }
    }
}
