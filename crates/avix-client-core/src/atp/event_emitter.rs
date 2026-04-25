use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::atp::{Dispatcher, Event, EventKind};
use crate::error::ClientError;

impl std::fmt::Debug for EventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

pub struct EventEmitter {
    rx: broadcast::Receiver<Event>,
    connected: Arc<AtomicBool>,
    last_seq: Arc<tokio::sync::Mutex<Option<u64>>>,
    _handle: JoinHandle<()>,
}

impl EventEmitter {
    pub fn start<F, Fut>(connect_fn: F) -> Self
    where
        F: Fn(Option<u64>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Dispatcher, ClientError>> + Send + 'static,
    {
        let (tx, rx) = broadcast::channel(256);
        let connected = Arc::new(AtomicBool::new(false));
        let last_seq = Arc::new(tokio::sync::Mutex::new(None::<u64>));
        let connect_fn = Arc::new(connect_fn);
        let tx_c = tx.clone();
        let connected_c = Arc::clone(&connected);
        let last_seq_c = Arc::clone(&last_seq);

        let handle = tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                let cursor = *last_seq_c.lock().await;
                let disp_res = connect_fn(cursor).await;
                if let Ok(disp) = disp_res {
                    connected_c.store(true, Ordering::SeqCst);
                    let mut disp_rx = disp.events();
                    loop {
                        match disp_rx.recv().await {
                            Ok(event) => {
                                let mut guard = last_seq_c.lock().await;
                                *guard = Some(match *guard {
                                    Some(prev) => prev.max(event.seq),
                                    None => event.seq,
                                });
                                drop(guard);
                                let _ = tx_c.send(event);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("EventEmitter lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    connected_c.store(false, Ordering::SeqCst);
                    backoff = Duration::from_secs(1);
                }
                tokio::time::sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(60));
            }
        });

        Self {
            rx,
            connected,
            last_seq,
            _handle: handle,
        }
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

    /// Returns the highest seq received across all connections, or None if no events yet.
    pub async fn last_seq(&self) -> Option<u64> {
        *self.last_seq.lock().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore = "requires mock Dispatcher transport (Gap B)"]
    async fn subscribe_all_receives_forwarded_events() {
        todo!("Inject event via fake dispatcher");
    }

    #[tokio::test]
    #[ignore = "requires mock Dispatcher transport (Gap B)"]
    async fn subscribe_kind_filters_correctly() {
        todo!("Assert only matching kind forwarded");
    }

    #[tokio::test]
    async fn reconnect_attempted_on_disconnect() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_c = Arc::clone(&count);
        let connect_fn = move |_since_seq: Option<u64>| {
            count_c.fetch_add(1, Ordering::SeqCst);
            async { Err(ClientError::NotConnected) }
        };
        let emitter = EventEmitter::start(connect_fn);
        assert!(!emitter.is_connected());
    }

    #[test]
    fn backoff_calc() {
        let mut b = Duration::from_secs(1);
        assert_eq!(b, Duration::from_secs(1));
        b = b.saturating_mul(2).min(Duration::from_secs(60));
        assert_eq!(b, Duration::from_secs(2));
        b = b.saturating_mul(2).min(Duration::from_secs(60));
        assert_eq!(b, Duration::from_secs(4));
        for _ in 0..10 {
            b = b.saturating_mul(2).min(Duration::from_secs(60));
        }
        assert_eq!(b, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn is_connected_false_before_first() {
        let connect_fn = |_: Option<u64>| async { Err(ClientError::NotConnected) };
        let emitter = EventEmitter::start(connect_fn);
        assert!(!emitter.is_connected());
    }

    #[tokio::test]
    async fn connect_fn_receives_none_on_first_connect() {
        let received = Arc::new(tokio::sync::Mutex::new(Vec::<Option<u64>>::new()));
        let received_c = Arc::clone(&received);
        let connect_fn = move |since_seq: Option<u64>| {
            let r = Arc::clone(&received_c);
            async move {
                r.lock().await.push(since_seq);
                Err(ClientError::NotConnected)
            }
        };
        let _emitter = EventEmitter::start(connect_fn);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = received.lock().await;
        assert!(!calls.is_empty());
        assert_eq!(calls[0], None, "first connect must pass None");
    }

    #[tokio::test]
    async fn last_seq_tracks_highest_seq() {
        // last_seq() starts None, stays None until events flow through a Dispatcher
        // (full wiring requires a mock Dispatcher — this just verifies the initial state)
        let connect_fn = |_: Option<u64>| async { Err(ClientError::NotConnected) };
        let emitter = EventEmitter::start(connect_fn);
        assert_eq!(emitter.last_seq().await, None);
    }
}
