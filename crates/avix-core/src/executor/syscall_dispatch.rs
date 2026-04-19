use std::path::Path;

use tracing::{debug, warn, instrument};

use crate::error::AvixError;
use crate::ipc::{
    frame,
    message::{JsonRpcRequest, JsonRpcResponse},
};
use crate::llm_svc::adapter::AvixToolCall;
use tokio::net::UnixStream;

/// Dispatch a kernel syscall tool call to the kernel IPC server.
///
/// Kernel tools (namespace `kernel/`) have no `IpcBinding` in their registry entry — they
/// are handled by the kernel's own IPC server (`AVIX_KERNEL_SOCK` / `runtime_dir/kernel.sock`).
/// `_caller` is always injected since kernel syscalls are always caller-scoped.
#[instrument(skip(call, runtime_dir))]
pub async fn dispatch_kernel_syscall(
    call: &AvixToolCall,
    agent_pid: u64,
    session_id: &str,
    runtime_dir: &Path,
) -> Result<serde_json::Value, AvixError> {
    // Resolve kernel socket path: env var override → runtime_dir/kernel.sock
    let socket_path = if let Ok(path) = std::env::var("AVIX_KERNEL_SOCK") {
        path
    } else {
        runtime_dir
            .join("kernel.sock")
            .to_string_lossy()
            .to_string()
    };

    // Kernel syscalls always receive _caller (they need to know which agent is calling)
    let mut params = call.args.clone();
    params["_caller"] = serde_json::json!({
        "pid": agent_pid,
        "session_id": session_id,
    });

    let rpc_req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: uuid::Uuid::new_v4().to_string(),
        method: call.name.clone(),
        params,
    };

    debug!(
        syscall = %call.name,
        socket = %socket_path,
        pid = agent_pid,
        "dispatching kernel syscall"
    );

    // ADR-05: fresh connection per call
    let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        warn!(syscall = %call.name, socket = %socket_path, error = %e, "kernel IPC connect failed");
        AvixError::Io(format!("kernel IPC connect to '{socket_path}' failed: {e}"))
    })?;

    frame::write_to(&mut stream, &rpc_req).await.map_err(|e| {
        warn!(syscall = %call.name, error = %e, "kernel IPC write failed");
        AvixError::Io(format!("kernel IPC write failed: {e}"))
    })?;

    let response: JsonRpcResponse = frame::read_from(&mut stream).await.map_err(|e| {
        warn!(syscall = %call.name, error = %e, "kernel IPC read failed");
        AvixError::Io(format!("kernel IPC read failed: {e}"))
    })?;

    if let Some(err) = response.error {
        warn!(
            syscall = %call.name,
            rpc_code = err.code,
            rpc_message = %err.message,
            "kernel syscall returned RPC error"
        );
        return Err(AvixError::Io(format!(
            "kernel syscall '{}' error {}: {}",
            call.name, err.code, err.message
        )));
    }

    debug!(syscall = %call.name, "kernel syscall completed");

    response
        .result
        .ok_or_else(|| AvixError::ConfigParse(format!("kernel syscall '{}' returned no result", call.name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;

    fn make_call(name: &str) -> AvixToolCall {
        AvixToolCall {
            call_id: "k-test".into(),
            name: name.into(),
            args: json!({"pid": 5}),
        }
    }

    async fn spawn_kernel_mock(
        sock_path: std::path::PathBuf,
        response: serde_json::Value,
    ) -> tokio::task::JoinHandle<serde_json::Value> {
        let listener = UnixListener::bind(&sock_path).unwrap();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let req: serde_json::Value = frame::read_from(&mut conn).await.unwrap();
            let resp = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": response,
            });
            let bytes = crate::ipc::frame::encode(&resp).unwrap();
            conn.write_all(&bytes).await.unwrap();
            req
        })
    }

    #[tokio::test]
    async fn test_kernel_syscall_routes_to_kernel_sock() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");

        let server = spawn_kernel_mock(sock, json!({"status": "ok"})).await;

        let call = make_call("kernel/proc/spawn");
        let result = dispatch_kernel_syscall(&call, 55, "sess-k", dir.path())
            .await
            .unwrap();

        assert_eq!(result["status"], "ok");

        // Verify _caller was injected
        let received = server.await.unwrap();
        assert_eq!(received["params"]["_caller"]["pid"], 55u64);
        assert_eq!(received["params"]["_caller"]["session_id"], "sess-k");
    }

    #[tokio::test]
    async fn test_kernel_syscall_connect_failure_no_socket() {
        let dir = tempfile::tempdir().unwrap();
        // Remove any env var that might override the socket path
        std::env::remove_var("AVIX_KERNEL_SOCK");

        // No listener at kernel.sock — should fail to connect
        let call = make_call("kernel/proc/spawn");
        let result = dispatch_kernel_syscall(&call, 1, "s", dir.path()).await;

        assert!(result.is_err(), "expected error, got: {:?}", result);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed") || err.contains("connect") || err.contains("No such file"),
            "got: {err}"
        );
    }
}
