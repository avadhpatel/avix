//! In-process signal channel registry.
//!
//! Replaces per-agent Unix sockets for kernel → executor signal delivery.
//! `IpcExecutorFactory` registers a sender at spawn and deregisters at exit.
//! `SignalHandler` calls [`SignalChannelRegistry::send`] instead of writing to a socket.
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use crate::signal::kind::Signal;
use crate::types::Pid;

/// Shared registry mapping agent PIDs to their inbound signal senders.
#[derive(Clone, Default)]
pub struct SignalChannelRegistry {
    inner: Arc<Mutex<HashMap<u32, mpsc::Sender<Signal>>>>,
}

impl SignalChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the sender for `pid`.  Overwrites any previous registration.
    pub async fn register(&self, pid: Pid, tx: mpsc::Sender<Signal>) {
        self.inner.lock().await.insert(pid.as_u32(), tx);
    }

    /// Deregister a previously registered PID (called at executor exit).
    pub async fn unregister(&self, pid: Pid) {
        self.inner.lock().await.remove(&pid.as_u32());
    }

    /// Send a signal to the registered executor for `pid`.
    ///
    /// Returns `true` if a channel existed and the send succeeded, `false` otherwise
    /// (agent not yet registered or has already exited).
    pub async fn send(&self, pid: Pid, signal: Signal) -> bool {
        let guard = self.inner.lock().await;
        if let Some(tx) = guard.get(&pid.as_u32()) {
            tx.send(signal).await.is_ok()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::kind::{Signal, SignalKind};
    use std::time::Duration;

    fn make_signal(pid: u32) -> Signal {
        Signal {
            target: Pid::new(pid),
            kind: SignalKind::Kill,
            payload: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn register_and_send_reaches_receiver() {
        let reg = SignalChannelRegistry::new();
        let (tx, mut rx) = mpsc::channel(8);
        let pid = Pid::new(10);

        reg.register(pid, tx).await;
        let sent = reg.send(pid, make_signal(10)).await;
        assert!(sent);

        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_eq!(received.kind, SignalKind::Kill);
    }

    #[tokio::test]
    async fn send_returns_false_for_unknown_pid() {
        let reg = SignalChannelRegistry::new();
        let sent = reg.send(Pid::new(99), make_signal(99)).await;
        assert!(!sent);
    }

    #[tokio::test]
    async fn unregister_removes_entry() {
        let reg = SignalChannelRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        let pid = Pid::new(20);

        reg.register(pid, tx).await;
        reg.unregister(pid).await;

        let sent = reg.send(pid, make_signal(20)).await;
        assert!(!sent, "send should fail after unregister");
    }

    #[tokio::test]
    async fn send_returns_false_when_receiver_dropped() {
        let reg = SignalChannelRegistry::new();
        let (tx, rx) = mpsc::channel(8);
        let pid = Pid::new(30);

        reg.register(pid, tx).await;
        drop(rx); // simulate executor exit before deregister

        let sent = reg.send(pid, make_signal(30)).await;
        assert!(!sent);
    }
}
