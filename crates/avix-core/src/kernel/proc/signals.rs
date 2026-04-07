use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::error::AvixError;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::process::ProcessTable;
use crate::process::entry::ProcessStatus;
use crate::session::{PersistentSessionStore, SessionStatus};
use crate::signal::{Signal, SignalDelivery, SignalKind};
use crate::types::Pid;

pub struct SignalHandler {
    runtime_dir: PathBuf,
    process_table: Arc<ProcessTable>,
    invocation_store: Option<Arc<InvocationStore>>,
    session_store: Option<Arc<PersistentSessionStore>>,
    active_invocations: Arc<Mutex<HashMap<u32, String>>>,
    active_sessions: Arc<Mutex<HashMap<u32, String>>>,
}

impl SignalHandler {
    pub fn new(
        runtime_dir: PathBuf,
        process_table: Arc<ProcessTable>,
        invocation_store: Option<Arc<InvocationStore>>,
        session_store: Option<Arc<PersistentSessionStore>>,
        active_invocations: Arc<Mutex<HashMap<u32, String>>>,
        active_sessions: Arc<Mutex<HashMap<u32, String>>>,
    ) -> Self {
        Self {
            runtime_dir,
            process_table,
            invocation_store,
            session_store,
            active_invocations,
            active_sessions,
        }
    }

    pub async fn pause_agent(&self, pid: u32) -> Result<(), AvixError> {
        let _ = self
            .process_table
            .set_status(Pid::new(pid), ProcessStatus::Paused)
            .await;

        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            let _ = istore.update_status(&id, InvocationStatus::Paused).await;
        }

        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let signal = Signal {
            target: Pid::new(pid),
            kind: SignalKind::Pause,
            payload: serde_json::Value::Null,
        };
        let _ = delivery.deliver(signal).await;

        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = uuid::Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if pid == session.owner_pid {
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::new(p))
                            .collect();
                        if !other_pids.is_empty() {
                            delivery
                                .broadcast(&other_pids, SignalKind::Pause, serde_json::Value::Null)
                                .await;
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Paused)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u32())
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
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn resume_agent(&self, pid: u32) -> Result<(), AvixError> {
        let _ = self
            .process_table
            .set_status(Pid::new(pid), ProcessStatus::Running)
            .await;

        let inv_id = self.active_invocations.lock().await.get(&pid).cloned();
        if let (Some(id), Some(istore)) = (inv_id, &self.invocation_store) {
            let _ = istore.update_status(&id, InvocationStatus::Running).await;
        }

        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let signal = Signal {
            target: Pid::new(pid),
            kind: SignalKind::Resume,
            payload: serde_json::Value::Null,
        };
        let _ = delivery.deliver(signal).await;

        let session_id_str = self.active_sessions.lock().await.get(&pid).cloned();
        if let (Some(sid), Some(sstore)) = (session_id_str, &self.session_store) {
            if let Ok(uuid) = uuid::Uuid::parse_str(&sid) {
                if let Ok(Some(mut session)) = sstore.get(&uuid).await {
                    if matches!(session.status, SessionStatus::Paused) {
                        let other_pids: Vec<Pid> = session
                            .pids
                            .iter()
                            .filter(|&&p| p != pid)
                            .map(|&p| Pid::new(p))
                            .collect();
                        if !other_pids.is_empty() {
                            delivery
                                .broadcast(&other_pids, SignalKind::Resume, serde_json::Value::Null)
                                .await;
                            for &sibling in &other_pids {
                                let _ = self
                                    .process_table
                                    .set_status(sibling, ProcessStatus::Running)
                                    .await;
                                let sibling_inv = self
                                    .active_invocations
                                    .lock()
                                    .await
                                    .get(&sibling.as_u32())
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
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn send_signal(
        &self,
        pid: u32,
        signal: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
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
                return Err(AvixError::ConfigParse(format!(
                    "unknown signal: {other}"
                )))
            }
        };
        let delivery = SignalDelivery::new(self.runtime_dir.clone());
        let sig = Signal {
            target: Pid::new(pid),
            kind,
            payload,
        };
        let _ = delivery.deliver(sig).await;
        Ok(())
    }
}