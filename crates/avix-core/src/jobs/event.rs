use serde::{Deserialize, Serialize};
use tracing::instrument;

use super::job::{JobError, JobState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Events emitted by a job during its lifetime.
/// Serialised with a `type` discriminant for JSON transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    StatusChange {
        job_id: String,
        old_state: JobState,
        new_state: JobState,
    },
    Progress {
        job_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stage: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Log {
        job_id: String,
        stream: LogStream,
        line: String,
    },
    Complete {
        job_id: String,
        result: serde_json::Value,
    },
    Fail {
        job_id: String,
        error: JobError,
    },
}

impl JobEvent {
    #[instrument]
    pub fn job_id(&self) -> &str {
        match self {
            Self::StatusChange { job_id, .. } => job_id,
            Self::Progress { job_id, .. } => job_id,
            Self::Log { job_id, .. } => job_id,
            Self::Complete { job_id, .. } => job_id,
            Self::Fail { job_id, .. } => job_id,
        }
    }
}
