use crate::atp::types::{Cmd, Frame, LoginRequest, LoginResponse, Subscribe};
use crate::config::ClientConfig;
use crate::error::ClientError;
use futures_util::{SinkExt, StreamExt};
use http::Request;
use reqwest;
use serde_json;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

pub type WsSink =
    futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
pub type WsStream = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

#[derive(Debug)]
pub struct AtpClient {
    pub session: LoginResponse,
    pub sink: WsSink,
    pub stream: WsStream,
}

impl AtpClient {
    pub async fn connect(config: ClientConfig) -> Result<Self, ClientError> {
        // Login first
        let login_url = format!("{}/atp/auth/login", config.server_url);
        debug!("Login POST {}", login_url);
        let login_req = LoginRequest {
            identity: config.identity,
            credential: config.credential,
        };
        let client = reqwest::Client::new();
        let res = client.post(&login_url).json(&login_req).send().await?;
        let login_resp: LoginResponse = res.json().await?;
        debug!("Login resp {:?}", login_resp);
        info!(session_id = %login_resp.session_id, "Logged in to ATP server");

        // WS connect
        let ws_url = config.server_url.replace("http", "ws") + "/atp";
        debug!("WS connect {}", ws_url);
        let req = Request::builder()
            .uri(&ws_url)
            .header("Authorization", format!("Bearer {}", login_resp.token))
            .body(())
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        let (ws_stream, _) = connect_async(req)
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;
        info!("WebSocket connected to ATP server");

        let (mut sink, stream) = ws_stream.split();

        let subscribe = Subscribe {
            frame_type: "subscribe".to_string(),
            events: vec!["*".to_string()],
        };

        sink.send(Message::Text(serde_json::to_string(&subscribe)?))
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        Ok(Self {
            session: login_resp,
            sink,
            stream,
        })
    }

    pub async fn send(&mut self, cmd: &Cmd) -> Result<(), ClientError> {
        let text = serde_json::to_string(cmd).map_err(ClientError::Json)?;
        debug!("Send cmd {:?}", cmd);
        self.sink
            .send(Message::Text(text))
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;
        Ok(())
    }

    pub async fn next_frame(&mut self) -> Option<Result<Frame, ClientError>> {
        loop {
            let opt_msg_res = self.stream.next().await;
            let msg_res = match opt_msg_res {
                Some(res) => res,
                None => return None,
            };
            let msg = match msg_res {
                Ok(m) => m,
                Err(e) => return Some(Err(ClientError::WebSocket(e.to_string()))),
            };
            match msg {
                Message::Text(text) => match serde_json::from_str(&text) {
                    Ok(frame) => {
                        debug!("Recv frame {:?}", frame);
                        return Some(Ok(frame));
                    }
                    Err(e) => return Some(Err(ClientError::Json(e))),
                },
                Message::Ping(p) => {
                    debug!("Received ping, sending pong");
                    if let Err(e) = self.sink.send(Message::Pong(p)).await {
                        warn!("Pong fail {:?}", e);
                    }
                }
                Message::Close(_) => {
                    debug!("WebSocket closed");
                    return None;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn connect_logs_in_success() {
        let mut server = Server::new_async().await;
        let login_resp = LoginResponse {
            token: "test-token".to_string(),
            expires_at: "2023-12-31".to_string(),
            session_id: "sess-1".to_string(),
        };
        let _m = server
            .mock("POST", "/atp/auth/login")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::to_string(&login_resp).unwrap())
            .create_async()
            .await;

        let config = ClientConfig {
            server_url: server.url(),
            identity: "user".to_string(),
            credential: "pass".to_string(),
            runtime_root: std::path::PathBuf::from("/tmp"),
            auto_start_server: false,
        };

        let result = AtpClient::connect(config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be WebSocket error, since login succeeded
        match err {
            ClientError::WebSocket(_) => {}
            _ => panic!("Expected WebSocket error, got {:?}", err),
        }
        // Check that the mock was called
        _m.assert_async().await;
    }

    #[tokio::test]
    async fn connect_fails_wrong_cred() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("POST", "/atp/auth/login")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error": "invalid credentials"}"#)
            .create_async()
            .await;

        let config = ClientConfig {
            server_url: server.url(),
            identity: "user".to_string(),
            credential: "wrong".to_string(),
            runtime_root: std::path::PathBuf::from("/tmp"),
            auto_start_server: false,
        };

        let result = AtpClient::connect(config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be Http error
        match err {
            ClientError::Http(_) => {}
            _ => panic!("Expected Http error, got {:?}", err),
        }
        _m.assert_async().await;
    }
}
