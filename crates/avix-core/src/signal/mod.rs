pub mod bus;
pub mod channels;
pub mod kind;
pub mod pipe_payload;

pub use bus::SignalBus;
pub use channels::SignalChannelRegistry;
pub use kind::{Signal, SignalKind};
pub use pipe_payload::{InlineEncoding, PipeAttachment, SigPipePayload};
