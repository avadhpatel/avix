use crate::error::AvixError;
use crate::ipc::{frame, message::IpcMessage, message::JsonRpcResponse};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::watch;

/// Handle to cancel a running `IpcServer`.
#[derive(Clone)]
pub struct IpcServerHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl IpcServerHandle {
    /// Signal the server to stop accepting new connections and drain in-flight calls.
    pub fn cancel(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// A single-connection-per-call IPC server over a Unix domain socket.
///
/// Each accepted connection is handled in an independent tokio task.
/// One request is read per connection; a response (if any) is written; connection closes.
pub struct IpcServer {
    path: PathBuf,
    listener: UnixListener,
    shutdown_rx: watch::Receiver<bool>,
}

impl IpcServer {
    /// Bind to `path`. Removes a stale socket file if one already exists.
    /// Returns the server and a handle that can be used to cancel it.
    pub async fn bind(path: PathBuf) -> Result<(Self, IpcServerHandle), AvixError> {
        // Remove stale socket from a previous run.
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                AvixError::Io(format!(
                    "failed to remove stale socket {}: {e}",
                    path.display()
                ))
            })?;
        }

        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AvixError::Io(format!(
                    "failed to create socket directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let listener = UnixListener::bind(&path).map_err(|e| {
            AvixError::Io(format!("failed to bind socket {}: {e}", path.display()))
        })?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = IpcServerHandle { shutdown_tx };
        Ok((
            Self {
                path,
                listener,
                shutdown_rx,
            },
            handle,
        ))
    }

    /// The socket path this server is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Start serving. Runs until the associated `IpcServerHandle::cancel` is called.
    ///
    /// `handler` is called for every incoming message.  Return `Some(response)` to send a
    /// reply; return `None` to close the connection silently (used for notifications).
    pub async fn serve<F, Fut>(mut self, handler: F) -> Result<(), AvixError>
    where
        F: Fn(IpcMessage) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<JsonRpcResponse>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let mut join_set = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                res = self.listener.accept() => {
                    match res {
                        Ok((conn, _)) => {
                            let h = handler.clone();
                            join_set.spawn(handle_connection(conn, h));
                        }
                        Err(e) => {
                            tracing::warn!("IpcServer accept error: {e}");
                        }
                    }
                }
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        break;
                    }
                }
            }
            // Reap any finished tasks eagerly.
            while join_set.try_join_next().is_some() {}
        }

        // Drain all in-flight connection tasks before returning.
        while join_set.join_next().await.is_some() {}
        Ok(())
    }
}

async fn handle_connection<F, Fut>(conn: tokio::net::UnixStream, handler: Arc<F>)
where
    F: Fn(IpcMessage) -> Fut + Send + Sync,
    Fut: Future<Output = Option<JsonRpcResponse>> + Send,
{
    let (mut read_half, mut write_half) = conn.into_split();

    // Read one frame as a raw JSON value to inspect the `id` key.
    let raw: serde_json::Value = match frame::read_from(&mut read_half).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("IpcServer: failed to read frame: {e}");
            return;
        }
    };

    let msg = match IpcMessage::from_value(raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("IpcServer: failed to parse IPC message: {e}");
            return;
        }
    };

    if let Some(response) = handler(msg).await {
        if let Err(e) = frame::write_to(&mut write_half, &response).await {
            tracing::warn!("IpcServer: failed to write response: {e}");
        }
    }
    // Connection drops here, closing both halves.
}
