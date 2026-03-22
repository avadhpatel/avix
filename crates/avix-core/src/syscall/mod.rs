use crate::types::token::CapabilityToken;

pub mod domain;
pub mod handler;

pub use handler::SyscallHandler;

#[derive(Debug, Clone)]
pub struct SyscallContext {
    pub caller_pid: u32,
    pub token: CapabilityToken,
}

#[derive(Debug, thiserror::Error)]
pub enum SyscallError {
    #[error("EPERM: caller {0} not authorized for {1}")]
    Eperm(u32, String),
    #[error("ENOENT: {0} not found")]
    Enoent(String),
    #[error("EINVAL: {0}")]
    Einval(String),
    #[error("EEXIST: {0} already exists")]
    Eexist(String),
}

pub type SyscallResult = Result<serde_json::Value, SyscallError>;
