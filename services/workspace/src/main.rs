//! workspace.svc — service entry point and registration

use std::path::PathBuf;

use serde_json::json;
use tracing::{debug, error, info};

use avix_core::ipc::message::{JsonRpcRequest, JsonRpcResponse};
use avix_core::ipc::server::IpcServer;

mod error;
mod handlers;

pub use error::WorkspaceError;
use handlers::{
    CreateProjectParams, DeleteParams, ListParams, ReadParams, SearchParams, SetDefaultParams,
    SnapshotParams, WorkspaceHandlers, WriteParams,
};

#[allow(unused_imports)]
use handlers::{
    CreateProjectResponse, DeleteResponse, InfoResponse, ListResponse, ReadResponse,
    SearchResponse, SetDefaultResponse, SnapshotResponse, WriteResponse,
};

pub struct WorkspaceService {
    handlers: WorkspaceHandlers,
}

impl WorkspaceService {
    pub fn new(kernel_sock: PathBuf) -> Self {
        let handlers = WorkspaceHandlers::new(kernel_sock);
        Self { handlers }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = req.id.clone();
        let method = req.method.clone();
        let params = req.params.clone();

        debug!(method = %method, "workspace request");

        self.handlers.set_caller_from_params(&params);

        let result = match method.as_str() {
            "workspace.list" => {
                let params: ListParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_list(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.read" => {
                let params: ReadParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_read(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.info" => match self.handlers.handle_info().await {
                Ok(resp) => json!(resp),
                Err(e) => {
                    return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                }
            },
            "workspace.write" => {
                let params: WriteParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_write(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.delete" => {
                let params: DeleteParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_delete(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.create-project" => {
                let params: CreateProjectParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_create_project(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.snapshot" => {
                let params: SnapshotParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_snapshot(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.search" => {
                let params: SearchParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_search(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            "workspace.set-default" => {
                let params: SetDefaultParams = match serde_json::from_value(params) {
                    Ok(p) => p,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            &id,
                            -32602,
                            &format!("invalid params: {e}"),
                            None,
                        ));
                    }
                };
                match self.handlers.handle_set_default(params).await {
                    Ok(resp) => json!(resp),
                    Err(e) => {
                        return Some(JsonRpcResponse::err(&id, -32000, &e.to_string(), None));
                    }
                }
            }
            _ => {
                return Some(JsonRpcResponse::err(
                    &id,
                    -32601,
                    &format!("method not found: {method}"),
                    None,
                ));
            }
        };

        Some(JsonRpcResponse::ok(&id, result))
    }
}

impl Clone for WorkspaceService {
    fn clone(&self) -> Self {
        Self {
            handlers: WorkspaceHandlers::new(PathBuf::from("/run/avix/kernel.sock")),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .init();

    let token = std::env::var("AVIX_SVC_TOKEN").expect("AVIX_SVC_TOKEN must be set");
    let kernel_sock_path = std::env::var("AVIX_KERNEL_SOCK").expect("AVIX_KERNEL_SOCK must be set");
    let svc_sock_path = std::env::var("AVIX_SVC_SOCK").expect("AVIX_SVC_SOCK must be set");

    info!("workspace.svc starting, token={}", &token[..8]);

    let kernel_sock = PathBuf::from(&kernel_sock_path);
    let svc_sock = PathBuf::from(&svc_sock_path);

    let client = avix_core::ipc::IpcClient::new(kernel_sock.clone());
    let register_req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: "1".into(),
        method: "ipc.register".into(),
        params: json!({
            "_token": token,
            "name": "workspace",
            "endpoint": svc_sock_path,
            "tools": []
        }),
    };

    let resp = client.call(register_req).await?;
    if resp.error.is_some() {
        error!("registration failed: {:?}", resp.error);
        return Err("registration failed".into());
    }
    info!("registered with kernel");

    let service = WorkspaceService::new(kernel_sock);

    let (server, _handle) = IpcServer::bind(svc_sock).await?;
    info!("listening on {}", svc_sock_path);

    server
        .serve(move |msg| {
            let svc = service.clone();
            async move {
                match msg {
                    avix_core::ipc::message::IpcMessage::Request(req) => {
                        svc.handle_request(req).await
                    }
                    avix_core::ipc::message::IpcMessage::Notification(_) => None,
                }
            }
        })
        .await?;

    Ok(())
}
