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

    #[error("no provider available for modality: {0}")]
    NoProviderAvailable(String),

    #[error("provider not permitted: {0}")]
    ProviderNotPermitted(String),

    #[error("adapter error: {0}")]
    AdapterError(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("IPC call timed out")]
    IpcTimeout,

    #[error("IPC frame too large")]
    IpcFrameTooLarge,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("manifest not found at path: {path}")]
    ManifestNotFound { path: String },

    #[error("manifest signature mismatch at path: {path}")]
    ManifestSignatureMismatch { path: String },

    #[error("manifest kind mismatch: expected '{expected}', found '{found}'")]
    ManifestKindMismatch { expected: String, found: String },

    #[error("required tool '{tool}' denied for agent '{agent}'")]
    RequiredToolDenied { tool: String, agent: String },

    #[error("model requirements not met: {reason}")]
    ModelRequirementsNotMet { reason: String },

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("MCP protocol error: {0}")]
    McpProtocol(String),

    #[error("MCP server unreachable: {0}")]
    McpUnreachable(String),

    /// Returned when an in-flight operation is cancelled by a signal (e.g. SIGKILL).
    #[error("cancelled: {0}")]
    Cancelled(String),
}

impl From<serde_json::Error> for AvixError {
    fn from(e: serde_json::Error) -> Self {
        AvixError::Serialization(e.to_string())
    }
}

impl From<std::io::Error> for AvixError {
    fn from(e: std::io::Error) -> Self {
        AvixError::Io(e.to_string())
    }
}

impl From<uuid::Error> for AvixError {
    fn from(e: uuid::Error) -> Self {
        AvixError::InvalidInput(e.to_string())
    }
}
