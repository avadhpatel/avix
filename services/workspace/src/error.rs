//! workspace.svc — error types

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("path outside workspace: {0}")]
    PathOutsideWorkspace(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("file not found: {0}")]
    FileNotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl From<avix_core::error::AvixError> for WorkspaceError {
    fn from(e: avix_core::error::AvixError) -> Self {
        WorkspaceError::Ipc(e.to_string())
    }
}
