use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{info, warn};

use crate::config::LlmConfig;
use crate::error::AvixError;
use crate::ipc::message::{IpcMessage, JsonRpcResponse};
use crate::ipc::{IpcServer, IpcServerHandle};
use crate::llm_client::LlmClient;
use crate::llm_svc::adapter::ProviderAdapter;
use crate::llm_svc::routing::RoutingEngine;
use crate::llm_svc::service::LlmService;

/// IPC server for `llm.svc`.
///
/// Listens on `llm.sock` and dispatches `llm/*` requests to `LlmService`.
/// Stale sockets from previous runs are removed automatically by `IpcServer::bind`.
pub struct LlmIpcServer {
    sock_path: PathBuf,
    service: Arc<LlmService>,
}

impl LlmIpcServer {
    pub fn new(
        sock_path: PathBuf,
        config: LlmConfig,
        adapters: HashMap<String, Box<dyn ProviderAdapter>>,
        routing: Arc<RoutingEngine>,
        text_clients: HashMap<String, Box<dyn LlmClient>>,
    ) -> Self {
        let service = Arc::new(LlmService::new(config, adapters, routing, text_clients));
        Self { sock_path, service }
    }

    /// Bind the socket, remove any stale file from a previous run, and start
    /// serving in a background task.  Returns an `IpcServerHandle` that can
    /// be dropped to shut the server down gracefully.
    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        info!(sock = %self.sock_path.display(), "llm IPC server bound");

        let svc = self.service;
        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let s = Arc::clone(&svc);
                    async move { handle_message(msg, s).await }
                })
                .await
            {
                warn!(error = %e, "llm IPC server exited");
            }
        });

        Ok(handle)
    }
}

async fn handle_message(msg: IpcMessage, svc: Arc<LlmService>) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => Some(svc.dispatch(&req).await),
        IpcMessage::Notification(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::message::{IpcMessage, JsonRpcNotification, JsonRpcRequest};
    use crate::ipc::IpcClient;
    use crate::llm_svc::routing::RoutingEngine;
    use tempfile::TempDir;

    fn make_minimal_config() -> LlmConfig {
        LlmConfig::from_str(
            r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: ollama
    image: ollama
    speech: ollama
    transcription: ollama
    embedding: ollama
  providers:
    - name: ollama
      baseUrl: http://localhost:11434
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: none
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn llm_ipc_server_binds_socket() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("llm.sock");
        let config = make_minimal_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let _handle = LlmIpcServer::new(sock.clone(), config, HashMap::new(), routing, HashMap::new())
            .start()
            .await
            .unwrap();
        assert!(sock.exists(), "socket file should exist after bind");
    }

    #[tokio::test]
    async fn llm_ipc_server_removes_stale_socket_on_restart() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("llm_stale.sock");

        // First binding
        let config = make_minimal_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let handle1 = LlmIpcServer::new(
            sock.clone(),
            config.clone(),
            HashMap::new(),
            Arc::new(RoutingEngine::from_config(&config)),
            HashMap::new(),
        )
        .start()
        .await
        .unwrap();
        assert!(sock.exists());
        drop(handle1);

        // Second binding on the same path must succeed (stale socket removed).
        let routing2 = Arc::new(RoutingEngine::from_config(&config));
        let _handle2 =
            LlmIpcServer::new(sock.clone(), config, HashMap::new(), routing2, HashMap::new())
                .start()
                .await
                .expect("second bind should succeed after stale socket removal");
        assert!(sock.exists());
    }

    #[tokio::test]
    async fn unknown_method_returns_error_over_ipc() {
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("llm_unk.sock");
        let config = make_minimal_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let _handle =
            LlmIpcServer::new(sock.clone(), config, HashMap::new(), routing, HashMap::new())
                .start()
                .await
                .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = IpcClient::new(sock.clone());
        let resp = client
            .call(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: "l1".into(),
                method: "llm/does-not-exist".into(),
                params: serde_json::json!({}),
            })
            .await
            .unwrap();

        assert!(resp.error.is_some(), "unknown method must return an error");
    }

    #[tokio::test]
    async fn notification_returns_none() {
        let config = make_minimal_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let svc = Arc::new(LlmService::new(config, HashMap::new(), routing, HashMap::new()));

        let notif = IpcMessage::Notification(JsonRpcNotification::new(
            "llm/ping",
            serde_json::json!({}),
        ));
        let result = handle_message(notif, svc).await;
        assert!(result.is_none(), "notifications should return None");
    }
}
