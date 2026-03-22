/// `job/watch` tool handler — polling model.
///
/// The caller passes a `job_id` and `timeout_ms`. This function returns the next
/// event for that job within the timeout, or `{ "status": "timeout" }` if none arrive,
/// or `{ "status": "not_found" }` if the job ID is unknown.
///
/// The agent calls this in a loop until it receives a `Complete` or `Fail` event.
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::error::AvixError;
use crate::jobs::job::JobState;
use crate::jobs::registry::JobRegistry;

const DEFAULT_WATCH_TIMEOUT_MS: u64 = 5_000;

pub async fn handle_job_watch(
    job_id: String,
    timeout_ms: Option<u64>,
    registry: Arc<RwLock<JobRegistry>>,
) -> Result<serde_json::Value, AvixError> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_WATCH_TIMEOUT_MS));

    // Subscribe first, then check state, to avoid missing events between check and subscribe.
    let mut rx = registry.read().await.subscribe();

    // If the job doesn't exist, return not_found immediately.
    {
        let reg = registry.read().await;
        if reg.get(&job_id).is_err() {
            return Ok(serde_json::json!({ "status": "not_found" }));
        }

        // If the job is already in a terminal state, emit the appropriate final event directly.
        let job = reg.get(&job_id).unwrap();
        match &job.state {
            JobState::Done => {
                let result = job.result.clone().unwrap_or(serde_json::Value::Null);
                return Ok(serde_json::json!({
                    "event": {
                        "type": "complete",
                        "job_id": job_id,
                        "result": result,
                    }
                }));
            }
            JobState::Failed => {
                if let Some(err) = &job.error {
                    return Ok(serde_json::json!({
                        "event": {
                            "type": "fail",
                            "job_id": job_id,
                            "error": {
                                "code": err.code,
                                "message": err.message,
                            }
                        }
                    }));
                }
            }
            JobState::Cancelled => {
                return Ok(serde_json::json!({
                    "event": {
                        "type": "status_change",
                        "job_id": job_id,
                        "old_state": "running",
                        "new_state": "cancelled",
                    }
                }));
            }
            _ => {}
        }
    }

    // Wait for the next matching event or timeout.
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(serde_json::json!({ "status": "timeout" }));
        }

        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(event)) if event.job_id() == job_id => {
                let event_json = serde_json::to_value(&event)
                    .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
                return Ok(serde_json::json!({ "event": event_json }));
            }
            Ok(Ok(_)) => {
                // Event for a different job; continue waiting.
                continue;
            }
            Ok(Err(_)) => {
                // Broadcast channel lagged or closed.
                return Ok(serde_json::json!({ "status": "timeout" }));
            }
            Err(_) => {
                // Timeout elapsed.
                return Ok(serde_json::json!({ "status": "timeout" }));
            }
        }
    }
}
