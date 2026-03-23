use std::sync::Arc;
use tokio::sync::broadcast;

use crate::gateway::atp::frame::AtpEvent;

const BUS_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct AtpEventBus {
    tx: Arc<broadcast::Sender<AtpEvent>>,
}

impl AtpEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx: Arc::new(tx) }
    }

    pub fn publish(&self, event: AtpEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AtpEvent> {
        self.tx.subscribe()
    }
}

impl Default for AtpEventBus {
    fn default() -> Self {
        Self::new()
    }
}
