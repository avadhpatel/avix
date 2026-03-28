//! Service IPC server — listens on a Unix socket and dispatches `ipc.*` methods
//! to `ServiceManager`. Services call this to register, add tools, and remove tools.
//!
//! Architecture invariants (ADR-05, ADR-06):
//! - Fresh connection per call; no multiplexing.
//! - 4-byte LE length-prefix framing handled by `crate::ipc::frame`.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::error::AvixError;
use crate::ipc::message::{IpcMessage, JsonRpcResponse};
use crate::ipc::{IpcServer, IpcServerHandle};
use crate::service::lifecycle::{
    IpcRegisterRequest, IpcToolAddParams, IpcToolRemoveParams, ServiceManager,
};

pub struct ServiceIpcServer {
    sock_path: PathBuf,
    service_manager: Arc<ServiceManager>,
    /// Root directory used to scan tool descriptors on `ipc.register`.
    avix_root: PathBuf,
}

impl ServiceIpcServer {
    pub fn new(
        sock_path: PathBuf,
        service_manager: Arc<ServiceManager>,
        avix_root: PathBuf,
    ) -> Self {
        Self {
            sock_path,
            service_manager,
            avix_root,
        }
    }

    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        info!(sock = %self.sock_path.display(), "service IPC server bound");

        let svc_mgr = Arc::clone(&self.service_manager);
        let root = self.avix_root.clone();

        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let mgr = Arc::clone(&svc_mgr);
                    let root = root.clone();
                    async move { handle_message(msg, mgr, root).await }
                })
                .await
            {
                warn!(error = %e, "service IPC server exited");
            }
        });

        Ok(handle)
    }
}

async fn handle_message(
    msg: IpcMessage,
    mgr: Arc<ServiceManager>,
    avix_root: PathBuf,
) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => {
            debug!(method = %req.method, id = %req.id, "service IPC request");
            let resp = dispatch_request(&req.id, &req.method, req.params, mgr, avix_root).await;
            Some(resp)
        }
        IpcMessage::Notification(notif) => {
            debug!(method = %notif.method, "service IPC notification (ignored)");
            None
        }
    }
}

async fn dispatch_request(
    id: &str,
    method: &str,
    params: serde_json::Value,
    mgr: Arc<ServiceManager>,
    avix_root: PathBuf,
) -> JsonRpcResponse {
    match method {
        "ipc.register" => {
            let token = match params["_token"].as_str() {
                Some(t) => t.to_string(),
                None => {
                    return JsonRpcResponse::err(id, -32602, "missing _token", None);
                }
            };
            let name = match params["name"].as_str() {
                Some(n) => n.to_string(),
                None => {
                    return JsonRpcResponse::err(id, -32602, "missing name", None);
                }
            };
            let endpoint = params["endpoint"].as_str().unwrap_or("").to_string();
            let tools = params["tools"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            match mgr
                .handle_ipc_register(
                    IpcRegisterRequest {
                        token,
                        name,
                        endpoint,
                        tools,
                    },
                    &avix_root,
                )
                .await
            {
                Ok(result) => {
                    info!(pid = result.pid.as_u32(), "service registered via IPC");
                    JsonRpcResponse::ok(
                        id,
                        json!({ "registered": result.registered, "pid": result.pid.as_u32() }),
                    )
                }
                Err(e) => {
                    warn!(error = %e, "ipc.register failed");
                    JsonRpcResponse::err(id, -32001, &e.to_string(), None)
                }
            }
        }

        "ipc.tool-add" => {
            let tool_params: IpcToolAddParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::err(id, -32602, &format!("invalid params: {e}"), None);
                }
            };
            match mgr.handle_tool_add(tool_params).await {
                Ok(()) => JsonRpcResponse::ok(id, json!({ "added": true })),
                Err(e) => {
                    warn!(error = %e, "ipc.tool-add failed");
                    JsonRpcResponse::err(id, -32001, &e.to_string(), None)
                }
            }
        }

        "ipc.tool-remove" => {
            let remove_params: IpcToolRemoveParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::err(id, -32602, &format!("invalid params: {e}"), None);
                }
            };
            match mgr.handle_tool_remove(remove_params).await {
                Ok(()) => JsonRpcResponse::ok(id, json!({ "removed": true })),
                Err(e) => {
                    warn!(error = %e, "ipc.tool-remove failed");
                    JsonRpcResponse::err(id, -32001, &e.to_string(), None)
                }
            }
        }

        other => {
            warn!(method = other, "service IPC: unknown method");
            JsonRpcResponse::err(
                id,
                -32601,
                &format!("unknown service IPC method: {other}"),
                None,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_manager(dir: &TempDir) -> Arc<ServiceManager> {
        Arc::new(ServiceManager::new_for_test(dir.path().to_path_buf()))
    }

    #[tokio::test]
    async fn dispatch_ipc_register_missing_token_returns_error() {
        let dir = TempDir::new().unwrap();
        let mgr = make_manager(&dir);
        let resp = dispatch_request(
            "req-1",
            "ipc.register",
            json!({ "name": "svc" }),
            mgr,
            dir.path().to_path_buf(),
        )
        .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn dispatch_ipc_tool_add_invalid_token_returns_error() {
        let dir = TempDir::new().unwrap();
        let mgr = make_manager(&dir);
        let resp = dispatch_request(
            "req-1",
            "ipc.tool-add",
            json!({ "_token": "bad", "tools": [] }),
            mgr,
            dir.path().to_path_buf(),
        )
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn dispatch_ipc_tool_remove_invalid_token_returns_error() {
        let dir = TempDir::new().unwrap();
        let mgr = make_manager(&dir);
        let resp = dispatch_request(
            "req-1",
            "ipc.tool-remove",
            json!({ "_token": "bad", "tools": [] }),
            mgr,
            dir.path().to_path_buf(),
        )
        .await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn dispatch_unknown_method_returns_32601() {
        let dir = TempDir::new().unwrap();
        let mgr = make_manager(&dir);
        let resp = dispatch_request(
            "req-1",
            "ipc.unknown",
            json!({}),
            mgr,
            dir.path().to_path_buf(),
        )
        .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn dispatch_ipc_tool_add_with_valid_token_succeeds() {
        let dir = TempDir::new().unwrap();
        let (mgr, _reg) = ServiceManager::new_with_registry(dir.path().to_path_buf());
        let mgr = Arc::new(mgr);

        let token = mgr
            .spawn_and_get_token(crate::service::lifecycle::ServiceSpawnRequest::simple(
                "test-svc",
                "/bin/test-svc",
            ))
            .await
            .unwrap();

        let resp = dispatch_request(
            "req-1",
            "ipc.tool-add",
            json!({
                "_token": token.token_str,
                "tools": [{ "name": "test/echo", "descriptor": { "description": "echo" } }]
            }),
            mgr,
            dir.path().to_path_buf(),
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["added"], true);
    }
}
