use std::collections::HashMap;
use std::sync::Arc;

use anyhow::anyhow;
use futures_util::SinkExt;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{debug, info};

use crate::atp::client::{AtpClient, WsSink};
use crate::atp::types::{Cmd, Event, Frame, Reply};
use crate::error::ClientError;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
struct DispatcherInner {
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Reply>>>>,
    event_tx: broadcast::Sender<Event>,
    session_id: String,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").finish_non_exhaustive()
    }
}

pub struct Dispatcher {
    inner: Arc<DispatcherInner>,
    sink_mutex: Arc<Mutex<WsSink>>,
    event_rx: broadcast::Receiver<Event>,
    _reader_handle: JoinHandle<()>,
}

impl Dispatcher {
    pub fn new(client: AtpClient) -> Self {
        let (event_tx, event_rx) = broadcast::channel(1024);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let session_id = client.session.session_id.clone();
        let inner = Arc::new(DispatcherInner {
            pending: pending.clone(),
            event_tx: event_tx.clone(),
            session_id,
        });

        let sink_mutex = Arc::new(Mutex::new(client.sink));
        let mut reader_stream = client.stream;
        let reader_inner = Arc::clone(&inner);

        let handle = tokio::spawn(async move {
            use futures_util::StreamExt as _;
            loop {
                let opt = reader_stream.next().await;
                let frame: Frame = match opt {
                    None => break,
                    Some(Err(e)) => {
                        tracing::error!("Dispatcher reader error: {:?}", e);
                        break;
                    }
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!("Frame parse error: {:?}", e);
                            continue;
                        }
                    },
                    Some(Ok(Message::Close(_))) => break,
                    Some(Ok(_)) => continue,
                };
                match frame {
                    Frame::Reply(reply) => {
                        debug!("Reply {:?}", reply);
                        let mut pending = reader_inner.pending.lock().await;
                        if let Some(tx) = pending.remove(&reply.id) {
                            let _ = tx.send(reply);
                        }
                    }
                    Frame::Event(event) => {
                        debug!("Event {:?}", event);
                        let _ = reader_inner.event_tx.send(event);
                    }
                }
            }
        });

        Self {
            inner,
            sink_mutex,
            event_rx,
            _reader_handle: handle,
        }
    }

    pub async fn call(&self, cmd: &Cmd) -> Result<Reply, ClientError> {
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(cmd.id.clone(), tx);

        info!("Dispatch cmd {:?} session={}", cmd, self.inner.session_id);
        let text = serde_json::to_string(cmd).map_err(ClientError::Json)?;
        let mut sink = self.sink_mutex.lock().await;
        sink.send(Message::Text(text))
            .await
            .map_err(|e| ClientError::WebSocket(e.to_string()))?;

        drop(sink);

        timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|e| ClientError::Other(anyhow!("oneshot cancelled: {}", e)))
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.event_rx.resubscribe()
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    #[ignore = "requires mock WS transport (Gap B)"]
    async fn call_returns_matching_reply() {
        todo!("Mock transport and test reply routing");
    }

    #[tokio::test]
    #[ignore = "requires mock WS transport (Gap B)"]
    async fn call_returns_error_on_not_ok_reply() {
        todo!("Inject bad reply, assert ClientError::Atp");
    }

    #[tokio::test]
    #[ignore = "requires mock WS transport (Gap B)"]
    async fn event_broadcast_reaches_subscriber() {
        todo!("Inject event, assert received via events()");
    }

    #[tokio::test]
    #[ignore = "requires mock WS transport (Gap B)"]
    async fn call_times_out_if_no_reply() {
        todo!("No reply injected, assert ClientError::Timeout");
    }
}
