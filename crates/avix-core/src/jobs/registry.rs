use std::collections::HashMap;

use chrono::Utc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::AvixError;
use crate::types::Pid;

use super::event::{JobEvent, LogStream};
use super::job::{Job, JobError, JobState};

const EVENT_CAPACITY: usize = 256;

pub struct JobRegistry {
    jobs: HashMap<String, Job>,
    events: broadcast::Sender<JobEvent>,
}

/// A receiver for all job events.
pub struct JobEventReceiver(pub broadcast::Receiver<JobEvent>);

impl JobRegistry {
    pub fn new() -> (Self, JobEventReceiver) {
        let (tx, rx) = broadcast::channel(EVENT_CAPACITY);
        (
            Self {
                jobs: HashMap::new(),
                events: tx,
            },
            JobEventReceiver(rx),
        )
    }

    /// Create a new job in `Pending` state. Returns the new job ID.
    pub fn create(&mut self, tool: &str, owner_pid: Pid) -> String {
        let id = format!("job-{}", Uuid::new_v4());
        let now = Utc::now();
        let job = Job {
            id: id.clone(),
            tool: tool.to_string(),
            owner_pid,
            state: JobState::Pending,
            created_at: now,
            updated_at: now,
            result: None,
            error: None,
        };
        self.jobs.insert(id.clone(), job);
        id
    }

    /// Transition `Pending` → `Running`.
    pub fn start(&mut self, job_id: &str) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state != JobState::Pending {
            return Err(AvixError::ConfigParse(format!(
                "cannot start job '{job_id}': state is {:?}",
                job.state
            )));
        }
        let old = job.state.clone();
        job.state = JobState::Running;
        job.updated_at = Utc::now();
        self.emit(JobEvent::StatusChange {
            job_id: job_id.to_string(),
            old_state: old,
            new_state: JobState::Running,
        });
        Ok(())
    }

    /// Emit a progress event without changing state. Job must be `Running`.
    pub fn progress(
        &mut self,
        job_id: &str,
        percent: Option<u8>,
        stage: Option<String>,
        detail: Option<String>,
    ) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state != JobState::Running {
            return Err(AvixError::ConfigParse(format!(
                "cannot emit progress for job '{job_id}': not running"
            )));
        }
        job.updated_at = Utc::now();
        self.emit(JobEvent::Progress {
            job_id: job_id.to_string(),
            percent,
            stage,
            detail,
        });
        Ok(())
    }

    /// Emit a log line event. Job must be `Running`.
    pub fn log(&mut self, job_id: &str, stream: LogStream, line: String) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state != JobState::Running {
            return Err(AvixError::ConfigParse(format!(
                "cannot emit log for job '{job_id}': not running"
            )));
        }
        job.updated_at = Utc::now();
        self.emit(JobEvent::Log {
            job_id: job_id.to_string(),
            stream,
            line,
        });
        Ok(())
    }

    /// Transition `Running` → `Done` and emit a `Complete` event.
    pub fn complete(
        &mut self,
        job_id: &str,
        result: serde_json::Value,
    ) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state.is_terminal() {
            return Err(AvixError::ConfigParse(format!(
                "cannot complete job '{job_id}': already in terminal state {:?}",
                job.state
            )));
        }
        let old = job.state.clone();
        job.state = JobState::Done;
        job.result = Some(result.clone());
        job.updated_at = Utc::now();
        self.emit(JobEvent::StatusChange {
            job_id: job_id.to_string(),
            old_state: old,
            new_state: JobState::Done,
        });
        self.emit(JobEvent::Complete {
            job_id: job_id.to_string(),
            result,
        });
        Ok(())
    }

    /// Transition `Running` → `Failed` and emit a `Fail` event.
    pub fn fail(&mut self, job_id: &str, error: JobError) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state.is_terminal() {
            return Err(AvixError::ConfigParse(format!(
                "cannot fail job '{job_id}': already in terminal state {:?}",
                job.state
            )));
        }
        let old = job.state.clone();
        job.state = JobState::Failed;
        job.error = Some(error.clone());
        job.updated_at = Utc::now();
        self.emit(JobEvent::StatusChange {
            job_id: job_id.to_string(),
            old_state: old,
            new_state: JobState::Failed,
        });
        self.emit(JobEvent::Fail {
            job_id: job_id.to_string(),
            error,
        });
        Ok(())
    }

    /// Cancel a `Running` or `Paused` job.
    pub fn cancel(&mut self, job_id: &str) -> Result<(), AvixError> {
        let job = self.get_mut(job_id)?;
        if job.state.is_terminal() {
            return Err(AvixError::ConfigParse(format!(
                "cannot cancel job '{job_id}': already in terminal state {:?}",
                job.state
            )));
        }
        let old = job.state.clone();
        job.state = JobState::Cancelled;
        job.updated_at = Utc::now();
        self.emit(JobEvent::StatusChange {
            job_id: job_id.to_string(),
            old_state: old,
            new_state: JobState::Cancelled,
        });
        Ok(())
    }

    /// Get the current state of a job.
    pub fn get(&self, job_id: &str) -> Result<&Job, AvixError> {
        self.jobs
            .get(job_id)
            .ok_or_else(|| AvixError::NotFound(format!("job '{job_id}' not found")))
    }

    /// Subscribe to all future job events.
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.events.subscribe()
    }

    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    // ── private ──────────────────────────────────────────────────────────────

    fn get_mut(&mut self, job_id: &str) -> Result<&mut Job, AvixError> {
        self.jobs
            .get_mut(job_id)
            .ok_or_else(|| AvixError::NotFound(format!("job '{job_id}' not found")))
    }

    fn emit(&self, event: JobEvent) {
        // Ignore send errors — no subscribers is fine.
        let _ = self.events.send(event);
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new().0
    }
}
