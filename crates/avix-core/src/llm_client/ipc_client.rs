use crate::ipc::frame;
use crate::ipc::message::{JsonRpcRequest, JsonRpcResponse};
use crate::llm_client::{LlmClient, LlmCompleteRequest, LlmCompleteResponse};
use async_trait::async_trait;
use tokio::net::UnixStream;

/// LlmClient implementation that calls llm.svc over IPC (Unix domain socket).
/// Uses fresh connection per call (ADR-05).
pub struct IpcLlmClient {
    /// Path to the llm.svc Unix socket
    pub socket_path: String,
    pub agent_pid: u32,
    pub session_id: String,
}

impl IpcLlmClient {
    pub fn new(
        socket_path: impl Into<String>,
        agent_pid: u32,
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
        // Build JSON-RPC params — embed agent metadata
        let params = serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "tools": req.tools,
            "system": req.system,
            "maxTokens": req.max_tokens,
            "metadata": {
                "agentPid": self.agent_pid,
                "sessionId": self.session_id,
            }
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

        // The result from llm.svc matches LlmCompleteResponse fields
        let resp: LlmCompleteResponse = serde_json::from_value(result)
            .map_err(|e| anyhow::anyhow!("failed to deserialize llm.svc response: {e}"))?;

        Ok(resp)
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
