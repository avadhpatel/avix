use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use tracing::instrument;

#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error("SIGPIPE: pipe {0} is closed")]
    Closed(String),
    #[error("pipe {0} is full (backpressure)")]
    Full(String),
}

pub struct Pipe {
    pub id: String,
    pub owner_pid: u64,
    tx: mpsc::Sender<String>,
    rx: Mutex<mpsc::Receiver<String>>,
    closed: AtomicBool,
}

impl std::fmt::Debug for Pipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipe")
            .field("id", &self.id)
            .field("owner_pid", &self.owner_pid)
            .field("closed", &self.closed)
            .finish()
    }
}

impl Pipe {
    #[instrument]
    pub fn new(owner_pid: u64, capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self {
            id: Uuid::new_v4().to_string(),
            owner_pid,
            tx,
            rx: Mutex::new(rx),
            closed: AtomicBool::new(false),
        }
    }

    #[instrument]
    pub async fn write(&self, msg: String) -> Result<(), PipeError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PipeError::Closed(self.id.clone()));
        }
        self.tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Closed(_) => PipeError::Closed(self.id.clone()),
            mpsc::error::TrySendError::Full(_) => PipeError::Full(self.id.clone()),
        })
    }

    #[instrument]
    /// Send a message, awaiting buffer space (Block backpressure policy).
    pub async fn send_blocking(&self, msg: String) -> Result<(), PipeError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PipeError::Closed(self.id.clone()));
        }
        self.tx
            .send(msg)
            .await
            .map_err(|_| PipeError::Closed(self.id.clone()))
    }

    #[instrument]
    pub async fn read(&self) -> Option<String> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
    }

    #[instrument]
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    #[instrument]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipe_new_is_open() {
        let pipe = Pipe::new(1, 8);
        assert!(!pipe.is_closed());
        assert!(!pipe.id.is_empty());
        assert_eq!(pipe.owner_pid, 1);
    }

    #[test]
    fn test_pipe_close_sets_closed_flag() {
        let pipe = Pipe::new(2, 8);
        assert!(!pipe.is_closed());
        pipe.close();
        assert!(pipe.is_closed());
    }

    #[tokio::test]
    async fn test_pipe_write_and_read() {
        let pipe = Pipe::new(3, 8);
        pipe.write("hello world".to_string()).await.unwrap();
        let msg = pipe.read().await;
        assert_eq!(msg, Some("hello world".to_string()));
    }

    #[tokio::test]
    async fn test_pipe_write_to_closed_returns_error() {
        let pipe = Pipe::new(4, 8);
        pipe.close();
        let result = pipe.write("message".to_string()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PipeError::Closed(id) => assert_eq!(id, pipe.id),
            other => panic!("expected Closed error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_pipe_write_full_returns_error() {
        // capacity of 1 — second write should hit Full
        let pipe = Pipe::new(5, 1);
        pipe.write("first".to_string()).await.unwrap();
        let result = pipe.write("second".to_string()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PipeError::Full(id) => assert_eq!(id, pipe.id),
            other => panic!("expected Full error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_pipe_error_display() {
        let e_closed = PipeError::Closed("pipe-123".to_string());
        assert!(e_closed.to_string().contains("pipe-123"));
        assert!(e_closed.to_string().contains("closed"));

        let e_full = PipeError::Full("pipe-456".to_string());
        assert!(e_full.to_string().contains("pipe-456"));
        assert!(e_full.to_string().contains("full"));
    }
}
