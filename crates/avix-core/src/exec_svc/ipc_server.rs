use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::error::AvixError;
use crate::exec_svc::ExecService;
use crate::ipc::message::{IpcMessage, JsonRpcResponse};
use crate::ipc::{IpcServer, IpcServerHandle};

const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

/// IPC server for `exec.svc`.
///
/// Listens on `exec.sock` and handles `exec/run` requests.
/// Request params: `{ "runtime": "python" | "node" | "bash", "code": "<source>" }`
/// Response body: `{ "stdout": "...", "stderr": "...", "exit_code": 0 }`
pub struct ExecIpcServer {
    sock_path: PathBuf,
}

impl ExecIpcServer {
    pub fn new(sock_path: PathBuf) -> Self {
        Self { sock_path }
    }

    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        info!(sock = %self.sock_path.display(), "exec IPC server bound");

        let svc = Arc::new(ExecService::new(DEFAULT_EXEC_TIMEOUT));
        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let s = Arc::clone(&svc);
                    async move { handle_message(msg, s).await }
                })
                .await
            {
                warn!(error = %e, "exec IPC server exited");
            }
        });

        Ok(handle)
    }
}

async fn handle_message(msg: IpcMessage, svc: Arc<ExecService>) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => {
            let resp = dispatch(&req.id, &req.method, &req.params, &svc).await;
            Some(resp)
        }
        IpcMessage::Notification(_) => None,
    }
}

async fn dispatch(
    id: &str,
    method: &str,
    params: &serde_json::Value,
    svc: &ExecService,
) -> JsonRpcResponse {
    match method {
        "exec/run" => {
            let runtime = params["runtime"].as_str().unwrap_or("bash");
            let code = params["code"].as_str().unwrap_or("");
            match svc.exec(runtime, code).await {
                Ok(r) => JsonRpcResponse::ok(
                    id,
                    serde_json::json!({
                        "stdout":    r.stdout,
                        "stderr":    r.stderr,
                        "exit_code": r.exit_code,
                    }),
                ),
                Err(e) => JsonRpcResponse::err(id, -32603, &e.to_string(), None),
            }
        }
        other => JsonRpcResponse::err(id, -32601, &format!("unknown method: {other}"), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::message::JsonRpcRequest;
    use crate::ipc::IpcClient;
    use tempfile::TempDir;

    #[tokio::test]
    async fn exec_ipc_server_binds_socket() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("exec.sock");
        let server = ExecIpcServer::new(sock.clone());
        let _handle = server.start().await.unwrap();
        assert!(sock.exists());
    }

    #[tokio::test]
    async fn exec_run_bash_returns_output() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("exec2.sock");
        let _handle = ExecIpcServer::new(sock.clone()).start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = IpcClient::new(sock.clone());
        let resp = client
            .call(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: "e1".into(),
                method: "exec/run".into(),
                params: serde_json::json!({ "runtime": "bash", "code": "echo hello" }),
            })
            .await
            .unwrap();

        assert!(resp.error.is_none(), "should succeed: {:?}", resp.error);
        let body = resp.result.unwrap();
        assert!(body["stdout"].as_str().unwrap_or("").contains("hello"));
        assert_eq!(body["exit_code"], 0);
    }

    #[tokio::test]
    async fn exec_run_unknown_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("exec3.sock");
        let _handle = ExecIpcServer::new(sock.clone()).start().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = IpcClient::new(sock.clone());
        let resp = client
            .call(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: "e2".into(),
                method: "exec/unknown".into(),
                params: serde_json::json!({}),
            })
            .await
            .unwrap();

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
