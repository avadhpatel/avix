use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use super::kind::{Signal, SignalKind};
use crate::types::Pid;

use tracing::instrument;

const CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

#[derive(Debug)]
pub struct Subscription {
    pub(crate) id: SubscriptionId,
    inner: broadcast::Receiver<Signal>,
}

impl Subscription {
    #[instrument]
    pub fn id(&self) -> SubscriptionId {
        self.id
    }

    #[instrument]
    pub async fn recv(&mut self) -> Option<Signal> {
        self.inner.recv().await.ok()
    }
}

#[derive(Debug)]
struct PidEntry {
    sender: broadcast::Sender<Signal>,
    sub_count: usize,
}

#[derive(Default, Debug)]
pub struct SignalBus {
    table: Arc<RwLock<HashMap<u64, PidEntry>>>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl SignalBus {
    #[instrument]
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument]
    pub async fn subscribe(&self, pid: Pid) -> Subscription {
        let id = SubscriptionId(
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let mut table = self.table.write().await;
        let entry = table.entry(pid.as_u64()).or_insert_with(|| PidEntry {
            sender: broadcast::channel(CHANNEL_CAPACITY).0,
            sub_count: 0,
        });
        entry.sub_count += 1;
        let rx = entry.sender.subscribe();
        Subscription { id, inner: rx }
    }

    #[instrument]
    pub async fn unsubscribe(&self, pid: Pid, _id: SubscriptionId) {
        let mut table = self.table.write().await;
        if let Some(entry) = table.get_mut(&pid.as_u64()) {
            entry.sub_count = entry.sub_count.saturating_sub(1);
            if entry.sub_count == 0 {
                table.remove(&pid.as_u64());
            }
        }
    }

    #[instrument]
    pub async fn send(&self, signal: Signal) -> Result<(), ()> {
        let table = self.table.read().await;
        if let Some(entry) = table.get(&signal.target.as_u64()) {
            let _ = entry.sender.send(signal);
        }
        Ok(())
    }

    #[instrument]
    pub async fn broadcast(&self, kind: SignalKind, payload: serde_json::Value) {
        let table = self.table.read().await;
        for (pid_u32, entry) in table.iter() {
            let sig = Signal {
                target: crate::types::Pid::from_u64(*pid_u32),
                kind: kind.clone(),
                payload: payload.clone(),
            };
            let _ = entry.sender.send(sig);
        }
    }

    #[instrument]
    pub async fn subscriber_count(&self, pid: Pid) -> usize {
        self.table
            .read()
            .await
            .get(&pid.as_u64())
            .map(|e| e.sub_count)
            .unwrap_or(0)
    }
}
