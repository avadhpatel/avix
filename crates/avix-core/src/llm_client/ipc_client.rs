use crate::ipc::frame;
use crate::ipc::message::{JsonRpcRequest, JsonRpcResponse};
use crate::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse, StreamChunk};
use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio::net::UnixStream;

/// LlmClient implementation that calls llm.svc over IPC (Unix domain socket).
/// Uses fresh connection per call (ADR-05).
pub struct IpcLlmClient {
    /// Path to the llm.svc Unix socket
    pub socket_path: String,
    pub agent_pid: u64,
    pub session_id: String,
}

impl IpcLlmClient {
    pub fn new(
        socket_path: impl Into<String>,
        agent_pid: u64,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            agent_pid,
            session_id: session_id.into(),
        }
    }
}

#[async_trait]
impl LlmClient for IpcLlmClient {
    async fn complete(&self, req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        // Serialize request with its natural snake_case field names, then add
        // agent metadata for routing / observability at the service end.
        let mut params =
            serde_json::to_value(&req).expect("LlmCompleteRequest is always serializable");
        params["metadata"] = serde_json::json!({
            "agent_pid": self.agent_pid,
            "session_id": self.session_id,
        });

        let rpc_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "llm/complete".into(),
            params,
        };

        // Fresh connection per call (ADR-05)
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| anyhow::anyhow!("IPC connect to llm.svc failed: {e}"))?;

        frame::write_to(&mut stream, &rpc_req)
            .await
            .map_err(|e| anyhow::anyhow!("IPC write failed: {e}"))?;

        let response: JsonRpcResponse = frame::read_from(&mut stream)
            .await
            .map_err(|e| anyhow::anyhow!("IPC read failed: {e}"))?;

        if let Some(err) = response.error {
            anyhow::bail!("llm.svc error {}: {}", err.code, err.message);
        }

        let result = response
            .result
            .ok_or_else(|| anyhow::anyhow!("llm.svc returned empty result"))?;

        tracing::debug!(result = ?result, "LLM.svc Result");

        // The result from llm.svc matches LlmCompleteResponse fields
        let resp: LlmCompleteResponse = serde_json::from_value(result)
            .map_err(|e| anyhow::anyhow!("failed to deserialize llm.svc response: {e}"))?;

        Ok(resp)
    }

    async fn stream_complete(
        &self,
        req: LlmCompleteRequest,
    ) -> anyhow::Result<BoxStream<'static, anyhow::Result<StreamChunk>>> {
        let mut params =
            serde_json::to_value(&req).expect("LlmCompleteRequest is always serializable");
        params["metadata"] = serde_json::json!({
            "agent_pid": self.agent_pid,
            "session_id": self.session_id,
        });

        let rpc_req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "llm/stream_complete".into(),
            params,
        };

        // Fresh connection per logical call (ADR-05 extended for streaming).
        let conn = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| anyhow::anyhow!("IPC connect to llm.svc failed: {e}"))?;

        let (mut read_half, mut write_half) = conn.into_split();

        frame::write_to(&mut write_half, &rpc_req)
            .await
            .map_err(|e| anyhow::anyhow!("IPC write failed: {e}"))?;

        // Spawn a task that reads frames from the socket and sends chunks to a channel.
        // The channel is bounded so we apply back-pressure if the consumer is slow.
        let (tx, rx) = tokio::sync::mpsc::channel::<anyhow::Result<StreamChunk>>(64);
        tokio::spawn(async move {
            // Keep write_half alive so the server doesn't see a half-close.
            let _write_half = write_half;
            loop {
                let raw: serde_json::Value = match frame::read_from(&mut read_half).await {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!("IPC read error: {e}"))).await;
                        return;
                    }
                };

                // Final response has "result" or "error" key; notifications have "method".
                let is_final = raw.get("result").is_some() || raw.get("error").is_some();

                if is_final {
                    if let Some(err) = raw.get("error") {
                        let msg = err["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                            .to_string();
                        let _ = tx
                            .send(Err(anyhow::anyhow!("llm.svc stream error: {msg}")))
                            .await;
                    }
                    return; // Channel dropped → stream ends for consumer.
                }

                let method = raw["method"].as_str().unwrap_or("");
                if method == "llm.stream.chunk" {
                    let chunk_val = raw["params"]["chunk"].clone();
                    match serde_json::from_value::<StreamChunk>(chunk_val) {
                        Ok(chunk) => {
                            if tx.send(Ok(chunk)).await.is_err() {
                                return; // Consumer dropped the stream.
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(Err(anyhow::anyhow!(
                                    "failed to deserialize StreamChunk: {e}"
                                )))
                                .await;
                            return;
                        }
                    }
                }
                // Unknown notifications are silently ignored.
            }
        });

        let chunk_stream = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });

        Ok(Box::pin(chunk_stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_client_new() {
        let client = IpcLlmClient::new("/tmp/llm.sock", 42, "session-abc");
        assert_eq!(client.socket_path, "/tmp/llm.sock");
        assert_eq!(client.agent_pid, 42);
        assert_eq!(client.session_id, "session-abc");
    }

    #[tokio::test]
    async fn test_ipc_client_connect_fails_gracefully() {
        let client = IpcLlmClient::new("/tmp/nonexistent-avix-test.sock", 1, "test-session");
        let req = LlmCompleteRequest {
            model: "claude-haiku-4-5-20251001".into(),
            messages: vec![],
            tools: vec![],
            system: None,
            max_tokens: 256,
            turn_id: uuid::Uuid::nil(),
        };
        let result = client.complete(req).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("IPC connect"),
            "expected 'IPC connect' in error: {msg}"
        );
    }
}
