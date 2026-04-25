use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{debug, info, warn, instrument};

use crate::error::AvixError;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::process::ProcessTable;
use crate::process::entry::ProcessStatus;
use crate::session::{PersistentSessionStore, SessionStatus};
use crate::signal::{Signal, SignalChannelRegistry, SignalKind};
use crate::types::Pid;

pub struct SignalHandler {
    channels: SignalChannelRegistry,
    process_table: Arc<ProcessTable>,
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
    active_invocations: Arc<Mutex<HashMap<u64, String>>>,
    active_sessions: Arc<Mutex<HashMap<u64, String>>>,
}

impl SignalHandler {
    pub fn new(
        channels: SignalChannelRegistry,
        process_table: Arc<ProcessTable>,
        invocation_store: Option<Arc<InvocationStore>>,
        session_store: Option<Arc<PersistentSessionStore>>,
        active_invocations: Arc<Mutex<HashMap<u64, String>>>,
        active_sessions: Arc<Mutex<HashMap<u64, String>>>,
    ) -> Self {
        Self {
            channels,
            process_table,
            invocation_store,
            session_store,
            active_invocations,
            active_sessions,
        }
    }

    /// Send a signal to one agent via its registered in-process channel.
    /// Returns `false` when no channel is registered for the pid.
    async fn deliver_to(&self, pid: Pid, kind: SignalKind, payload: serde_json::Value) -> bool {
        let sig = Signal { target: pid, kind, payload };
        if !self.channels.send(pid, sig).await {
            warn!(pid = pid.as_u64(), "no signal channel registered for agent (not running?)");
            return false;
        }
        true
    }

    /// Send a signal to multiple agents concurrently.
    async fn broadcast_to(&self, pids: &[Pid], kind: SignalKind, payload: serde_json::Value) {
        let futs: Vec<_> = pids
            .iter()
            .map(|&p| {
                let channels = self.channels.clone();
                let k = kind.clone();
                let v = payload.clone();
                async move {
                    let sig = Signal { target: p, kind: k, payload: v };
                    channels.send(p, sig).await;
                }
            })
            .collect();
        futures::future::join_all(futs).await;
    }

    #[instrument(skip(self))]
    pub async fn pause_agent(&self, pid: u64) -> Result<(), AvixError> {
        info!(pid, "pausing agent");

        let _ = self
            .process_table
            .set_status(Pid::from_u64(pid), ProcessStatus::Paused)
            .await;
        debug!(pid, "set process status to Paused");

        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            if let Err(e) = istore.update_status(&id, InvocationStatus::Paused).await {
                warn!(pid, invocation_id = %id, error = %e, "failed to update invocation status");
            }
            debug!(pid, "updated invocation status to Paused");
        }

        self.deliver_to(Pid::from_u64(pid), SignalKind::Pause, serde_json::Value::Null).await;
        debug!(pid, "delivered SIGPAUSE");

        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = uuid::Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if pid == session.owner_pid {
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::from_u64(p))
                            .collect();
                        if !other_pids.is_empty() {
                            info!(pid, sibling_count = other_pids.len(), "cascading pause to session participants");
                            self.broadcast_to(&other_pids, SignalKind::Pause, serde_json::Value::Null).await;
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Paused)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u64())
                                    .cloned();
                                if let (Some(iid), Some(istore)) =
                                    (sibling_inv, &self.invocation_store)
                                {
                                    let _ =
                                        istore.update_status(&iid, InvocationStatus::Paused).await;
                                }
                            }
                        }
                        session.mark_paused();
                        let _ = sstore.update(&session).await;
                        debug!(session_id = %session.id, "marked session as Paused");
                    }
                }
            }
        }
        info!(pid, "agent paused successfully");
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn resume_agent(&self, pid: u64) -> Result<(), AvixError> {
        info!(pid, "resuming agent");

        let _ = self
            .process_table
            .set_status(Pid::from_u64(pid), ProcessStatus::Running)
            .await;
        debug!(pid, "set process status to Running");

        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            if let Err(e) = istore.update_status(&id, InvocationStatus::Running).await {
                warn!(pid, invocation_id = %id, error = %e, "failed to update invocation status");
            }
            debug!(pid, "updated invocation status to Running");
        }

        self.deliver_to(Pid::from_u64(pid), SignalKind::Resume, serde_json::Value::Null).await;
        debug!(pid, "delivered SIGRESUME");

        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = uuid::Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if matches!(session.status, SessionStatus::Paused) {
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::from_u64(p))
                            .collect();
                        if !other_pids.is_empty() {
                            info!(pid, sibling_count = other_pids.len(), "cascading resume to session participants");
                            self.broadcast_to(&other_pids, SignalKind::Resume, serde_json::Value::Null).await;
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Running)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u64())
                                    .cloned();
                                if let (Some(iid), Some(istore)) =
                                    (sibling_inv, &self.invocation_store)
                                {
                                    let _ =
                                        istore.update_status(&iid, InvocationStatus::Running).await;
                                }
                            }
                        }
                        session.mark_running();
                        let _ = sstore.update(&session).await;
                        debug!(session_id = %session.id, "marked session as Running");
                    }
                }
            }
        }
        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn send_signal(
        &self,
        pid: u64,
        signal: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
        info!(pid, signal, "sending signal to agent");

        match signal {
            "SIGPAUSE" => return self.pause_agent(pid).await,
            "SIGRESUME" => return self.resume_agent(pid).await,
            _ => {}
        }
        let kind = match signal {
            "SIGSTART" => SignalKind::Start,
            "SIGKILL" => SignalKind::Kill,
            "SIGSTOP" => SignalKind::Stop,
            "SIGSAVE" => SignalKind::Save,
            "SIGPIPE" => SignalKind::Pipe,
            "SIGESCALATE" => SignalKind::Escalate,
            other => {
                warn!(signal = other, "unknown signal requested");
                return Err(AvixError::ConfigParse(format!("unknown signal: {other}")));
            }
        };
        if !self.deliver_to(Pid::from_u64(pid), kind, payload).await {
            return Err(AvixError::NotFound(format!("no signal channel for pid {pid}")));
        }
        debug!(pid, signal, "signal delivered successfully");
        Ok(())
    }
}
