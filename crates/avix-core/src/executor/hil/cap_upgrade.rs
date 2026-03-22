use crate::error::AvixError;
use crate::signal::kind::SignalKind;
use crate::signal::SignalBus;
use crate::types::{token::CapabilityToken, Pid};
use std::sync::Arc;
use std::time::Duration;

pub struct CapabilityUpgrader {
    pid: Pid,
    token: CapabilityToken,
    bus: Arc<SignalBus>,
}

impl CapabilityUpgrader {
    pub fn new(pid: Pid, token: CapabilityToken, bus: Arc<SignalBus>) -> Self {
        Self { pid, token, bus }
    }

    pub async fn request_tool(
        &mut self,
        _tool: &str,
        _reason: &str,
        hil_id: &str,
        timeout: Duration,
    ) -> Result<(), AvixError> {
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
        .map_err(|_| AvixError::CapabilityDenied("HIL timeout".into()))?;

        let decision = payload["decision"].as_str().unwrap_or("denied");
        if decision != "approved" {
            return Err(AvixError::CapabilityDenied(
                "capability upgrade denied".into(),
            ));
        }

        // Replace token from payload
        if let Ok(new_token) =
            serde_json::from_value::<CapabilityToken>(payload["new_capability_token"].clone())
        {
            self.token = new_token;
        }

        Ok(())
    }

    pub fn current_token(&self) -> &CapabilityToken {
        &self.token
    }
}
