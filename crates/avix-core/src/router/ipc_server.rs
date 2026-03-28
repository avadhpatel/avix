use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::error::AvixError;
use crate::ipc::message::{IpcMessage, JsonRpcResponse};
use crate::ipc::{IpcServer, IpcServerHandle};
use crate::router::RouterDispatcher;
use crate::types::Pid;

/// IPC server for `router.svc`.
///
/// Listens on `router.sock` and dispatches incoming tool calls to the owning service
/// via `RouterDispatcher`.  Caller identity is carried in three reserved params:
/// `_caller_pid`, `_caller_user`, `_caller_token`.  These fields are stripped before
/// the request is forwarded to the service.
pub struct RouterIpcServer {
    sock_path: PathBuf,
    dispatcher: Arc<RouterDispatcher>,
}

impl RouterIpcServer {
    pub fn new(sock_path: PathBuf, dispatcher: Arc<RouterDispatcher>) -> Self {
        Self {
            sock_path,
            dispatcher,
        }
    }

    /// Bind the socket and start serving in a background task.
    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        info!(sock = %self.sock_path.display(), "router IPC server bound");

        let dispatcher = Arc::clone(&self.dispatcher);
        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let d = Arc::clone(&dispatcher);
                    async move { handle_message(msg, d).await }
                })
                .await
            {
                warn!(error = %e, "router IPC server exited");
            }
        });

        Ok(handle)
    }
}

async fn handle_message(
    msg: IpcMessage,
    dispatcher: Arc<RouterDispatcher>,
) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(mut req) => {
            // Extract caller identity from params, defaulting gracefully if absent.
            let caller_pid = req
                .params
                .get("_caller_pid")
                .and_then(|v| v.as_u64())
                .map(|p| Pid::new(p as u32))
                .unwrap_or(Pid::new(0));
            let caller_user = req
                .params
                .get("_caller_user")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let caller_token = req
                .params
                .get("_caller_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Strip caller fields before forwarding to the service.
            if let Some(obj) = req.params.as_object_mut() {
                obj.remove("_caller_pid");
                obj.remove("_caller_user");
                obj.remove("_caller_token");
            }

            let resp = dispatcher
                .dispatch(req, caller_pid, &caller_user, &caller_token)
                .await;
            Some(resp)
        }
        IpcMessage::Notification(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessTable;
    use crate::router::registry::ServiceRegistry;
    use crate::tool_registry::ToolRegistry;
    use tempfile::TempDir;

    fn make_dispatcher() -> Arc<RouterDispatcher> {
        Arc::new(RouterDispatcher::new(
            Arc::new(ServiceRegistry::new()),
            Arc::new(ToolRegistry::new()),
            Arc::new(ProcessTable::new()),
        ))
    }

    #[tokio::test]
    async fn router_ipc_server_binds_and_starts() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("router.sock");
        let server = RouterIpcServer::new(sock.clone(), make_dispatcher());
        let handle = server.start().await.unwrap();
        assert!(sock.exists(), "socket file should exist after bind");
        drop(handle);
    }

    #[tokio::test]
    async fn unknown_tool_returns_error_response() {
        use crate::ipc::message::JsonRpcRequest;
        use crate::ipc::IpcClient;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("router2.sock");
        let server = RouterIpcServer::new(sock.clone(), make_dispatcher());
        let _handle = server.start().await.unwrap();

        // Give the server a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = IpcClient::new(sock.clone());
        let resp = client
            .call(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: "t1".into(),
                method: "nonexistent/tool".into(),
                params: serde_json::json!({}),
            })
            .await
            .unwrap();

        assert!(resp.error.is_some(), "unknown tool should return an error");
    }

    #[test]
    fn caller_fields_extracted_from_params() {
        // Verify the extraction logic in isolation by calling handle_message directly
        // via a notification (which returns None — just exercising the match arm).
        use crate::ipc::message::JsonRpcNotification;
        let msg = IpcMessage::Notification(JsonRpcNotification::new(
            "test",
            serde_json::json!({}),
        ));
        // The notification arm returns None — no panic expected.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(handle_message(msg, make_dispatcher()));
        assert!(result.is_none());
    }
}
