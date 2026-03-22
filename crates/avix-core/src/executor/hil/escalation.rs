use crate::error::AvixError;
use crate::signal::kind::SignalKind;
use crate::signal::SignalBus;
use crate::types::Pid;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct EscalationResult {
    pub selected_option: String,
    pub guidance: String,
}

pub struct Escalator {
    pid: Pid,
    bus: Arc<SignalBus>,
    pending_messages: Vec<String>,
}

impl Escalator {
    pub fn new(pid: Pid, bus: Arc<SignalBus>) -> Self {
        Self {
            pid,
            bus,
            pending_messages: Vec::new(),
        }
    }

    pub async fn escalate(
        &mut self,
        _situation: &str,
        _context: &str,
        _options: &[(&str, &str)],
        hil_id: &str,
        timeout: Duration,
    ) -> Result<EscalationResult, AvixError> {
        let mut rx = self.bus.subscribe(self.pid).await;
        let hil_id = hil_id.to_string();
        let payload = tokio::time::timeout(timeout, async move {
            loop {
                if let Some(sig) = rx.recv().await {
                    if sig.kind == SignalKind::Resume
                        && sig.payload["hilId"].as_str() == Some(&hil_id)
                    {
                        return sig.payload;
                    }
                }
            }
        })
        .await
        .map_err(|_| AvixError::CapabilityDenied("escalation timed out".into()))?;

        let selected_option = payload["selectedOption"].as_str().unwrap_or("").to_string();
        let guidance = payload["guidance"].as_str().unwrap_or("").to_string();

        self.pending_messages
            .push(format!("[Human guidance]: {guidance}"));

        Ok(EscalationResult {
            selected_option,
            guidance,
        })
    }

    pub fn pending_messages(&self) -> &[String] {
        &self.pending_messages
    }
}
