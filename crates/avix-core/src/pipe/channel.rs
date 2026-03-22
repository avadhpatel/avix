use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error("SIGPIPE: pipe {0} is closed")]
    Closed(String),
    #[error("pipe {0} is full (backpressure)")]
    Full(String),
}

pub struct Pipe {
    pub id: String,
    pub owner_pid: u32,
    tx: mpsc::Sender<String>,
    rx: Mutex<mpsc::Receiver<String>>,
    closed: AtomicBool,
}

impl Pipe {
    pub fn new(owner_pid: u32, capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        Self {
            id: Uuid::new_v4().to_string(),
            owner_pid,
            tx,
            rx: Mutex::new(rx),
            closed: AtomicBool::new(false),
        }
    }

    pub async fn write(&self, msg: String) -> Result<(), PipeError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PipeError::Closed(self.id.clone()));
        }
        self.tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Closed(_) => PipeError::Closed(self.id.clone()),
            mpsc::error::TrySendError::Full(_) => PipeError::Full(self.id.clone()),
        })
    }

    pub async fn read(&self) -> Option<String> {
        let mut rx = self.rx.lock().await;
        rx.recv().await
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}
