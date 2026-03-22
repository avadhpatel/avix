use thiserror::Error;

#[derive(Debug, Error)]
pub enum AvixError {
    #[error("invalid PID: {0}")]
    InvalidPid(String),

    #[error("invalid IPC address: {0}")]
    InvalidIpcAddr(String),

    #[error("invalid tool name '{name}': {reason}")]
    InvalidToolName { name: String, reason: String },

    #[error("unknown credential type: {0}")]
    UnknownCredentialType(String),

    #[error("config parse error: {0}")]
    ConfigParse(String),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}
