use crate::atp::types::{Cmd, Frame, LoginRequest, LoginResponse, Subscribe};
use crate::error::ClientError;
use futures_util::{SinkExt, StreamExt};
use http::{header, Request};
use serde_json;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub type WsSink =
    futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
pub type WsStream = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

pub struct AtpClient {
    pub session: LoginResponse,
    pub sink: WsSink,
    pub stream: WsStream,
}

impl AtpClient {
    pub async fn connect(url: &str, _user: &str, _token: &str) -> Result<Self, ClientError> {
        let ws_url = url;

        let req = Request::builder()
            .uri(ws_url)
            .body(())
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        let (ws_stream, _) = connect_async(req)
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        let (mut sink, stream) = ws_stream.split();

        let subscribe = Subscribe {
            frame_type: "subscribe".to_string(),
            events: vec!["*".to_string()],
        };

        sink.send(Message::Text(serde_json::to_string(&subscribe)?))
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        Ok(Self {
            session: LoginResponse {
                token: "dummy".to_string(),
                expires_at: "dummy".to_string(),
                session_id: "dummy".to_string(),
            },
            sink,
            stream,
        })
    }

    pub async fn send(&mut self, cmd: &Cmd) -> Result<(), ClientError> {
        let text = serde_json::to_string(cmd).map_err(ClientError::Json)?;
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
                Message::Text(text) => {
                    return Some(serde_json::from_str(&text).map_err(ClientError::Json));
                }
                Message::Ping(p) => {
                    let _ = self
                        .sink
                        .send(Message::Pong(p))
                        .await
                        .map_err(|e| ClientError::WebSocket(e.to_string()));
                }
                Message::Close(_) => return None,
                _ => {}
            }
        }
    }
}
