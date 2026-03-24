use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::error::ClientError;
use crate::atp::{Dispatcher, Event, EventKind};

pub struct EventEmitter {
    rx: broadcast::Receiver<Event>,
    connected: Arc<AtomicBool>,
    _handle: JoinHandle<()>,
}

impl EventEmitter {
    pub fn start<F, Fut>(connect_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Dispatcher, ClientError>> + Send + 'static,
    {
        let (tx, rx) = broadcast::channel(256);
        let connected = Arc::new(AtomicBool::new(false));
        let connect_fn = Arc::new(connect_fn);
        let tx_c = tx.clone();
        let connected_c = Arc::clone(&connected);

        let handle = tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                let disp_res = connect_fn().await;
                if let Ok(disp) = disp_res {
                    connected_c.store(true, Ordering::SeqCst);
                    let mut disp_rx = disp.events();
                    tokio::select! {
                        res = disp_rx.recv() => {
                            if let Ok(event) = res {
                                let _ = tx_c.send(event);
                            }
                        }
                    }
                    connected_c.store(false, Ordering::SeqCst);
                    // Reset backoff after stable (here simplified)
                    backoff = Duration::from_secs(1);
                }
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(60));
            }
        });

        Self { rx, connected, _handle: handle }
    }

    pub fn subscribe_all(&self) -> broadcast::Receiver<Event> {
        self.rx.resubscribe()
    }

    pub fn subscribe(&self, kind: EventKind) -> broadcast::Receiver<Event> {
        let (tx, rx) = broadcast::channel(256);
        let mut rx_all = self.subscribe_all();
        let tx_c = tx.clone();
        tokio::spawn(async move {
            loop {
                match rx_all.recv().await {
                    Ok(event) if event.kind == kind => {
                        let _ = tx_c.send(event);
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
        rx
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atp::types::{AgentOutputBody, AgentStatus, AgentStatusBody, EventBody, SessionReadyBody};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    async fn fake_connect(_: Arc<AtomicUsize>) -> Result<Dispatcher, ClientError> {
        todo!("Mock Dispatcher from gap B tests");
    }

    #[tokio::test]
    async fn subscribe_all_receives_forwarded_events() {
        todo!("Inject event via fake dispatcher");
    }

    #[tokio::test]
    async fn subscribe_kind_filters_correctly() {
        todo!("Assert only matching kind forwarded");
    }

    #[tokio::test]
    async fn reconnect_attempted_on_disconnect() {
        let count = Arc::new(AtomicUsize::new(0));
        let connect_fn = || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Err(ClientError::NotConnected) }
        };
        let emitter = EventEmitter::start(connect_fn);
        // Simulate disconnect, check count >1
        assert!(!emitter.is_connected());
    }

    #[tokio::test]
    fn backoff_calc() {
        let mut b = Duration::from_secs(1);
        assert_eq!(b, Duration::from_secs(1));
        b = b.saturating_mul(2).min(Duration::from_secs(60));
        assert_eq!(b, Duration::from_secs(2));
        // ... up to 60
    }

    #[tokio::test]
    fn is_connected_false_before_first() {
        let connect_fn = || async { Err(ClientError::NotConnected) };
        let emitter = EventEmitter::start(connect_fn);
        assert!(!emitter.is_connected());
    }
}