use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use tokio::net::unix::OwnedWriteHalf;
use tracing::{debug, info, warn};

use crate::config::LlmConfig;
use crate::error::AvixError;
use crate::ipc::frame;
use crate::ipc::message::{IpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
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
    ///
    /// Regular `llm/*` requests use request-response (ADR-05).
    /// `llm/stream_complete` requests keep the connection open, writing
    /// `llm.stream.chunk` notifications followed by a final response.
    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        info!(sock = %self.sock_path.display(), "llm IPC server bound");

        let svc = self.service;
        tokio::spawn(async move {
            if let Err(e) = server
                .serve_bidir(move |msg, write_half| {
                    let s = Arc::clone(&svc);
                    async move { handle_message_bidir(msg, s, write_half).await }
                })
                .await
            {
                warn!(error = %e, "llm IPC server exited");
            }
        });

        Ok(handle)
    }
}

/// Bi-directional handler: routes to streaming or regular path based on method.
async fn handle_message_bidir(
    msg: IpcMessage,
    svc: Arc<LlmService>,
    mut write_half: OwnedWriteHalf,
) {
    match msg {
        IpcMessage::Request(ref req) if req.method == "llm/stream_complete" => {
            handle_stream_complete(req.clone(), svc, &mut write_half).await;
        }
        IpcMessage::Request(req) => {
            let resp = svc.dispatch(&req).await;
            if let Err(e) = frame::write_to(&mut write_half, &resp).await {
                warn!(error = %e, "llm IPC: failed to write response");
            }
        }
        IpcMessage::Notification(_) => {}
    }
}

/// Streaming handler: calls `LlmService::stream_complete`, writes chunk
/// notifications on the open connection, then a final `JsonRpcResponse`.
async fn handle_stream_complete(
    req: JsonRpcRequest,
    svc: Arc<LlmService>,
    write_half: &mut OwnedWriteHalf,
) {
    let stream_result = svc.dispatch_stream(&req).await;
    let mut stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            let resp = JsonRpcResponse::err(&req.id, -32603, &e.to_string(), None);
            let _ = frame::write_to(write_half, &resp).await;
            return;
        }
    };

    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;
    let mut stop_reason_str = "end_turn".to_string();

    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                use crate::llm_client::StreamChunk;
                if let StreamChunk::Done {
                    stop_reason,
                    input_tokens: it,
                    output_tokens: ot,
                } = &chunk
                {
                    input_tokens = *it;
                    output_tokens = *ot;
                    stop_reason_str = serde_json::to_value(stop_reason)
                        .unwrap_or_default()
                        .as_str()
                        .unwrap_or("end_turn")
                        .to_string();
                }

                let notif = JsonRpcNotification::new(
                    "llm.stream.chunk",
                    serde_json::json!({ "stream_id": &req.id, "chunk": serde_json::to_value(&chunk).unwrap_or_default() }),
                );
                if let Err(e) = frame::write_to(write_half, &notif).await {
                    debug!(error = %e, "llm stream: client disconnected mid-stream");
                    return;
                }
            }
            Err(e) => {
                let resp = JsonRpcResponse::err(&req.id, -32603, &e.to_string(), None);
                let _ = frame::write_to(write_half, &resp).await;
                return;
            }
        }
    }

    // Final response summarises the completed stream.
    let resp = JsonRpcResponse::ok(
        &req.id,
        serde_json::json!({
            "done": true,
            "stop_reason": stop_reason_str,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }),
    );
    let _ = frame::write_to(write_half, &resp).await;
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
        let _handle = LlmIpcServer::new(
            sock.clone(),
            config,
            HashMap::new(),
            routing,
            HashMap::new(),
        )
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
        let _handle2 = LlmIpcServer::new(
            sock.clone(),
            config,
            HashMap::new(),
            routing2,
            HashMap::new(),
        )
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
        let _handle = LlmIpcServer::new(
            sock.clone(),
            config,
            HashMap::new(),
            routing,
            HashMap::new(),
        )
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
    async fn notification_is_silently_ignored() {
        // Notifications are silently dropped — the bidir handler returns ()
        // without writing any frame.  We verify this indirectly: send a
        // notification to the running server and confirm the connection closes
        // cleanly with no response frame.
        let dir = TempDir::new().unwrap();
        let sock = dir.path().join("llm_notif.sock");
        let config = make_minimal_config();
        let routing = Arc::new(RoutingEngine::from_config(&config));
        let _handle = LlmIpcServer::new(
            sock.clone(),
            config,
            HashMap::new(),
            routing,
            HashMap::new(),
        )
        .start()
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        use crate::ipc::frame;
        let mut conn = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let notif =
            IpcMessage::Notification(JsonRpcNotification::new("llm/ping", serde_json::json!({})));
        frame::write_to(&mut conn, &notif).await.unwrap();

        // Server should close the write side without sending a response.
        // Reading should return an error or empty (connection closed).
        let read_result: Result<serde_json::Value, _> = frame::read_from(&mut conn).await;
        assert!(
            read_result.is_err(),
            "server should close connection after notification without a response"
        );
    }
}
