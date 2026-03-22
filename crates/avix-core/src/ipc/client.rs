use crate::error::AvixError;
use crate::ipc::{
    frame, message::JsonRpcNotification, message::JsonRpcRequest, message::JsonRpcResponse,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::UnixStream;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5_000);

/// A stateless IPC client that opens a fresh Unix socket connection per call.
///
/// Follows ADR-05: no persistent multiplexed channels.
#[derive(Clone)]
pub struct IpcClient {
    target: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    /// Create a client that connects to `target`.
    pub fn new(target: PathBuf) -> Self {
        Self {
            target,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the per-call timeout (default 5 s).
    pub fn with_timeout(self, d: Duration) -> Self {
        Self { timeout: d, ..self }
    }

    /// Send a request and wait for a response.
    /// Opens a fresh connection, writes the request frame, reads the response frame, closes.
    pub async fn call(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse, AvixError> {
        let target = self.target.clone();
        let fut = async move {
            let mut conn = UnixStream::connect(&target).await.map_err(|e| {
                AvixError::Io(format!("IpcClient connect to {}: {e}", target.display()))
            })?;
            frame::write_to(&mut conn, &req).await?;
            let resp: JsonRpcResponse = frame::read_from(&mut conn).await?;
            Ok(resp)
        };

        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| AvixError::IpcTimeout)?
    }

    /// Send a notification (fire-and-forget — no response read).
    /// Opens a fresh connection, writes the frame, closes immediately.
    pub async fn notify(&self, notif: JsonRpcNotification) -> Result<(), AvixError> {
        let mut conn = UnixStream::connect(&self.target).await.map_err(|e| {
            AvixError::Io(format!(
                "IpcClient connect to {}: {e}",
                self.target.display()
            ))
        })?;
        frame::write_to(&mut conn, &notif).await?;
        Ok(())
    }
}
