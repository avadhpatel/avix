use std::path::Path;

use crate::error::AvixError;
use crate::ipc::{
    frame,
    message::{JsonRpcRequest, JsonRpcResponse},
};
use crate::llm_svc::adapter::AvixToolCall;
use tokio::net::UnixStream;
use tracing::{debug, warn};

/// Dispatch a Cat1 tool call over a local IPC socket (ADR-05: fresh connection per call).
///
/// `descriptor` is the tool's full descriptor JSON (from `ToolRegistry`). The `ipc` field
/// must be present and contain `endpoint` and `method` sub-fields.
///
/// If `caller_scoped` is true, a `_caller` object is injected into the request params
/// containing the agent PID and session ID (Architecture § _caller injection).
pub async fn dispatch_cat1_tool(
    call: &AvixToolCall,
    descriptor: &serde_json::Value,
    agent_pid: u64,
    session_id: &str,
    runtime_dir: &Path,
    caller_scoped: bool,
) -> Result<serde_json::Value, AvixError> {
    // 1. Extract IpcBinding from descriptor["ipc"]
    let ipc = match descriptor.get("ipc") {
        Some(v) if !v.is_null() => v,
        _ => {
            return Err(AvixError::ConfigParse(format!(
                "tool '{}' has no IPC binding",
                call.name
            )));
        }
    };

    let endpoint = ipc["endpoint"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("IPC binding missing endpoint".into()))?;
    let method = ipc["method"]
        .as_str()
        .ok_or_else(|| AvixError::ConfigParse("IPC binding missing method".into()))?;

    // 2. Resolve socket path: env var override first, then runtime_dir/<endpoint>.sock
    let env_key = format!(
        "AVIX_{}_SOCK",
        endpoint.to_uppercase().replace(['-', '.'], "_")
    );
    let socket_path = if let Ok(path) = std::env::var(&env_key) {
        path
    } else {
        runtime_dir
            .join(format!("{endpoint}.sock"))
            .to_string_lossy()
            .to_string()
    };

    // 3. Build params; inject _caller if the service is caller-scoped
    let mut params = call.args.clone();
    if caller_scoped {
        params["_caller"] = serde_json::json!({
            "pid": agent_pid,
            "session_id": session_id,
        });
    }

    // 4. Build and send JSON-RPC 2.0 request (ADR-05: fresh UnixStream per call)
    let rpc_req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: uuid::Uuid::new_v4().to_string(),
        method: method.to_string(),
        params,
    };

    debug!(
        tool = %call.name,
        socket = %socket_path,
        method,
        caller_scoped,
        "dispatching Cat1 tool over IPC"
    );

    let mut stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        warn!(tool = %call.name, socket = %socket_path, error = %e, "IPC connect failed");
        AvixError::Io(format!("IPC connect to '{socket_path}' failed: {e}"))
    })?;

    frame::write_to(&mut stream, &rpc_req).await.map_err(|e| {
        warn!(tool = %call.name, socket = %socket_path, error = %e, "IPC write failed");
        AvixError::Io(format!("IPC write to '{socket_path}' failed: {e}"))
    })?;

    // 7. Read length-prefixed response
    let response: JsonRpcResponse = frame::read_from(&mut stream).await.map_err(|e| {
        warn!(tool = %call.name, socket = %socket_path, error = %e, "IPC read failed");
        AvixError::Io(format!("IPC read from '{socket_path}' failed: {e}"))
    })?;

    // 5. Propagate JSON-RPC error field
    if let Some(err) = response.error {
        warn!(
            tool = %call.name,
            rpc_code = err.code,
            rpc_message = %err.message,
            "Cat1 tool returned RPC error"
        );
        return Err(AvixError::Io(format!(
            "tool '{}' RPC error {}: {}",
            call.name, err.code, err.message
        )));
    }

    debug!(tool = %call.name, "Cat1 tool dispatch completed");

    // 6. Return result
    response
        .result
        .ok_or_else(|| AvixError::ConfigParse(format!("tool '{}' IPC response had no result", call.name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;

    fn make_call(name: &str, args: serde_json::Value) -> AvixToolCall {
        AvixToolCall {
            call_id: "test-id".into(),
            name: name.into(),
            args,
        }
    }

    fn make_descriptor(endpoint: &str, method: &str) -> serde_json::Value {
        json!({
            "name": "fs/read",
            "description": "Read a file",
            "ipc": {
                "transport": "local-ipc",
                "endpoint": endpoint,
                "method": method
            }
        })
    }

    /// Spawn a mock UnixListener that reads one request and sends a canned JSON-RPC response.
    async fn spawn_mock_service(
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
    async fn test_successful_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test-svc.sock");
        let expected_result = json!({"content": "hello world"});

        let server = spawn_mock_service(sock.clone(), expected_result.clone()).await;

        let descriptor = make_descriptor("test-svc", "fs.read");
        let call = make_call("fs/read", json!({"path": "/data/file.txt"}));

        let result = dispatch_cat1_tool(&call, &descriptor, 42, "sess-1", dir.path(), false)
            .await
            .unwrap();

        assert_eq!(result, expected_result);
        let _ = server.await.unwrap();
    }

    #[tokio::test]
    async fn test_caller_injection_when_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("scoped-svc.sock");

        let server = spawn_mock_service(sock.clone(), json!({"ok": true})).await;

        let descriptor = make_descriptor("scoped-svc", "fs.write");
        let call = make_call("fs/write", json!({"path": "/f", "content": "x"}));

        dispatch_cat1_tool(&call, &descriptor, 99, "session-abc", dir.path(), true)
            .await
            .unwrap();

        // The server task returns the request it received; check _caller was injected
        let received_req = server.await.unwrap();
        let caller = &received_req["params"]["_caller"];
        assert_eq!(caller["pid"], 99u64);
        assert_eq!(caller["session_id"], "session-abc");
    }

    #[tokio::test]
    async fn test_no_caller_injection_when_not_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("unscoped-svc.sock");

        let server = spawn_mock_service(sock.clone(), json!({"ok": true})).await;

        let descriptor = make_descriptor("unscoped-svc", "fs.read");
        let call = make_call("fs/read", json!({}));

        dispatch_cat1_tool(&call, &descriptor, 77, "session-xyz", dir.path(), false)
            .await
            .unwrap();

        let received_req = server.await.unwrap();
        assert!(
            received_req["params"]["_caller"].is_null()
                || received_req["params"].get("_caller").is_none(),
            "_caller should not be present when not caller_scoped"
        );
    }

    #[tokio::test]
    async fn test_missing_ipc_binding_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let descriptor = json!({
            "name": "fs/read",
            "description": "Read a file"
            // no "ipc" field
        });
        let call = make_call("fs/read", json!({}));

        let result = dispatch_cat1_tool(&call, &descriptor, 1, "s", dir.path(), false).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no IPC binding"), "got: {err}");
    }

    #[tokio::test]
    async fn test_socket_connect_failure_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        // no listener on this socket
        let descriptor = make_descriptor("nonexistent-svc", "some.method");
        let call = make_call("some/tool", json!({}));

        let result =
            dispatch_cat1_tool(&call, &descriptor, 1, "s", dir.path(), false).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed") || err.contains("connect"), "got: {err}");
    }
}
