//! Avix Client Core — ATP protocol types, shared state, config.

pub mod atp;
pub mod error;

pub use crate::atp::*;
pub use crate::error::ClientError;

// Stub modules for future gaps
pub mod commands; // gap E
pub mod config; // gap E
pub mod notification; // gap D
pub mod persistence; // gap D
pub mod server; // gap E
pub mod state; // gap E
