/// Agent IPC socket lifecycle.
///
/// Each agent binds a Unix socket at `/run/avix/agents/<pid>.sock` that the kernel
/// uses to deliver inbound signals (SIGPAUSE, SIGRESUME, SIGKILL, …).
/// The socket is created at agent spawn and removed on shutdown.
use crate::error::AvixError;
use crate::ipc::{platform, IpcServer, IpcServerHandle};
use crate::types::Pid;
use std::path::Path;

/// Bind the agent's inbound signal socket.
///
/// Creates the parent `agents/` directory if needed.
/// Returns `(IpcServer, IpcServerHandle)` — the caller must call `.serve()` to begin
/// accepting signals, and may call `handle.cancel()` at shutdown.
pub async fn create_agent_socket(
    run_dir: &Path,
    pid: Pid,
) -> Result<(IpcServer, IpcServerHandle), AvixError> {
    let path = platform::agent_sock_path(run_dir, pid);
    IpcServer::bind(path).await
}

/// Remove the agent's signal socket file.
///
/// Silently succeeds if the file no longer exists (already cleaned up).
pub async fn remove_agent_socket(run_dir: &Path, pid: Pid) -> Result<(), AvixError> {
    let path = platform::agent_sock_path(run_dir, pid);
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| AvixError::Io(format!("failed to remove agent socket: {e}")))?;
    }
    Ok(())
}
