use crate::error::AvixError;
use crate::signal::kind::SignalKind;
use crate::signal::SignalBus;
use crate::types::Pid;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct ApprovalResult {
    pub approved: bool,
    pub note: Option<String>,
    pub denial_reason: Option<String>,
}

pub struct HilApprover {
    pid: Pid,
    bus: Arc<SignalBus>,
}

impl HilApprover {
    pub fn new(pid: Pid, bus: Arc<SignalBus>) -> Self {
        Self { pid, bus }
    }

    pub async fn await_approval(
        &self,
        hil_id: &str,
        timeout: Duration,
    ) -> Result<ApprovalResult, AvixError> {
        let mut rx = self.bus.subscribe(self.pid).await;
        let hil_id = hil_id.to_string();
        let result = tokio::time::timeout(timeout, async move {
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
        .map_err(|_| AvixError::CapabilityDenied("HIL approval timed out".into()))?;

        let decision = result["decision"].as_str().unwrap_or("denied");
        let approved = decision == "approved";
        Ok(ApprovalResult {
            approved,
            note: result["note"].as_str().map(|s| s.to_string()),
            denial_reason: result["reason"].as_str().map(|s| s.to_string()),
        })
    }
}
