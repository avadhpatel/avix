pub mod channel;
pub mod config;
pub mod manager;
pub mod registry;

pub use channel::{Pipe, PipeError};
pub use config::{BackpressurePolicy, PipeConfig, PipeDirection, PipeEncoding};
pub use manager::{PipeManager, ReadResult};
pub use registry::PipeRegistry;
