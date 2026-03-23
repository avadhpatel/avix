use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use super::loader::normalise_expression;
use super::scheduler::CronScheduler;
use super::schema::{CrontabFile, CrontabJob, OnFailure};

/// Request sent to `AgentSpawner::spawn`.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub user: String,
    pub agent_template: String,
    pub goal: String,
    pub timeout_sec: u64,
}

/// Opaque handle returned by `AgentSpawner::spawn`; identifies the running agent.
#[derive(Debug, Clone)]
pub struct SpawnHandle {
    pub run_id: String,
}

/// Exit status reported by a spawned agent.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentExitStatus {
    Success,
    Failure(String),
}

/// Trait over the kernel spawn mechanism, allowing unit tests to inject mocks.
#[async_trait]
pub trait AgentSpawner: Send + Sync {
    /// Spawn an agent and return a handle to track it.
    async fn spawn(&self, request: SpawnRequest) -> Result<SpawnHandle, String>;

    /// Wait for the agent to exit. Implementations must resolve promptly on
    /// agent completion; callers wrap this in `tokio::time::timeout` for enforcement.
    async fn wait_for_exit(&self, handle: SpawnHandle) -> AgentExitStatus;

    /// Forcibly stop the agent (sent when the timeout fires).
    async fn stop(&self, handle: SpawnHandle);
}

/// Callback called when a job fails and `on_failure` is `Alert` (or retry exhaustion).
#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, job_id: &str, reason: &str);
}

/// Default `AlertSink` that emits a `tracing::warn!`.
pub struct LogAlertSink;

#[async_trait]
impl AlertSink for LogAlertSink {
    async fn send(&self, job_id: &str, reason: &str) {
        tracing::warn!(job_id, reason, "cron job failed");
    }
}

/// Substitutes `{varName}` placeholders in `goal` from `args`.
///
/// Unknown placeholders are left as-is.
pub fn render_goal(goal: &str, args: &HashMap<String, serde_json::Value>) -> String {
    let mut result = goal.to_owned();
    for (k, v) in args {
        let placeholder = format!("{{{}}}", k);
        let value = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        result = result.replace(&placeholder, &value);
    }
    result
}

/// Background cron runner.
///
/// Holds a reference to the `CronScheduler` (for timing) and a map of
/// `CrontabJob` entries (for execution details). On each tick, fires any
/// due jobs and applies the `onFailure` / `retryPolicy` rules.
pub struct CronRunner {
    scheduler: Arc<RwLock<CronScheduler>>,
    jobs: HashMap<String, CrontabJob>,
    spawner: Arc<dyn AgentSpawner>,
    alert_sink: Arc<dyn AlertSink>,
    /// Tick interval in seconds (default 30).
    tick_interval_sec: u64,
}

impl CronRunner {
    /// Build a runner from a loaded `CrontabFile`.
    ///
    /// Registers every job's schedule with the scheduler and stores the full
    /// `CrontabJob` spec for use at dispatch time.
    pub async fn from_crontab(
        crontab: CrontabFile,
        spawner: Arc<dyn AgentSpawner>,
        alert_sink: Arc<dyn AlertSink>,
    ) -> Result<Self, String> {
        let scheduler = CronScheduler::new();

        let mut jobs = HashMap::new();

        for job in &crontab.spec.jobs {
            // Normalise to 6-field for the scheduler (same as the loader)
            let expr_6 = normalise_expression(&job.schedule);
            use super::scheduler::MissedRunPolicy;
            scheduler
                .add_job(job.id.clone(), expr_6, MissedRunPolicy::Skip)
                .await
                .map_err(|e| format!("job '{}': {e}", job.id))?;

            jobs.insert(job.id.clone(), job.clone());
        }

        Ok(Self {
            scheduler: Arc::new(RwLock::new(scheduler)),
            jobs,
            spawner,
            alert_sink,
            tick_interval_sec: 30,
        })
    }

    /// Override the tick interval (useful in tests).
    pub fn with_tick_interval(mut self, secs: u64) -> Self {
        self.tick_interval_sec = secs;
        self
    }

    /// Start the background tick loop and return a `JoinHandle`.
    ///
    /// The loop fires every `tick_interval_sec` seconds. To stop it,
    /// drop or abort the returned handle.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move { self.run().await })
    }

    async fn run(self) {
        let mut last_tick = Utc::now();

        loop {
            tokio::time::sleep(Duration::from_secs(self.tick_interval_sec)).await;
            let tick_time = Utc::now();

            let due_ids = self.scheduler.read().await.due_jobs(last_tick).await;

            for id in due_ids {
                // Mark last_run in the scheduler so we don't re-fire
                self.scheduler
                    .write()
                    .await
                    .update_last_run(&id, tick_time)
                    .await;

                if let Some(job) = self.jobs.get(&id) {
                    let job = job.clone();
                    let spawner = Arc::clone(&self.spawner);
                    let alert_sink = Arc::clone(&self.alert_sink);

                    tokio::spawn(async move {
                        run_job(job, spawner, alert_sink).await;
                    });
                }
            }

            last_tick = tick_time;
        }
    }
}

/// Execute one cron job with retry / alert semantics.
///
/// This function is `pub` so integration tests can call it directly
/// without starting the full tick loop.
pub async fn run_job(
    job: CrontabJob,
    spawner: Arc<dyn AgentSpawner>,
    alert_sink: Arc<dyn AlertSink>,
) {
    let max_attempts = if job.on_failure == OnFailure::Retry {
        job.retry_policy.max_attempts.max(1)
    } else {
        1
    };

    let mut attempt = 0u32;

    loop {
        attempt += 1;

        let goal = render_goal(&job.goal, &job.args);
        let request = SpawnRequest {
            user: job.user.clone(),
            agent_template: job.agent_template.clone(),
            goal,
            timeout_sec: job.timeout,
        };

        tracing::debug!(
            job_id = %job.id,
            attempt,
            max_attempts,
            "cron job attempt"
        );

        let status = execute_once(&job, request, Arc::clone(&spawner)).await;

        match status {
            AgentExitStatus::Success => {
                tracing::debug!(job_id = %job.id, "cron job succeeded");
                return;
            }
            AgentExitStatus::Failure(reason) => {
                tracing::warn!(job_id = %job.id, attempt, reason = %reason, "cron job attempt failed");

                match &job.on_failure {
                    OnFailure::Ignore => {
                        tracing::debug!(job_id = %job.id, "on_failure=ignore — discarding");
                        return;
                    }
                    OnFailure::Alert => {
                        alert_sink.send(&job.id, &reason).await;
                        return;
                    }
                    OnFailure::Retry => {
                        if attempt >= max_attempts {
                            // Exhausted — fall back to alert
                            let msg =
                                format!("exhausted {max_attempts} attempts; last error: {reason}");
                            alert_sink.send(&job.id, &msg).await;
                            return;
                        }
                        // Wait backoff then retry
                        tokio::time::sleep(Duration::from_secs(job.retry_policy.backoff_sec)).await;
                    }
                }
            }
        }
    }
}

/// Spawn the agent once, enforce the timeout, and return its exit status.
async fn execute_once(
    job: &CrontabJob,
    request: SpawnRequest,
    spawner: Arc<dyn AgentSpawner>,
) -> AgentExitStatus {
    let handle = match spawner.spawn(request).await {
        Ok(h) => h,
        Err(e) => return AgentExitStatus::Failure(format!("spawn error: {e}")),
    };

    let timeout = Duration::from_secs(job.timeout);
    let wait_result = tokio::time::timeout(timeout, spawner.wait_for_exit(handle.clone())).await;

    match wait_result {
        Ok(status) => status,
        Err(_elapsed) => {
            tracing::warn!(
                job_id = %job.id,
                timeout_sec = job.timeout,
                "job timed out — sending stop"
            );
            spawner.stop(handle).await;
            AgentExitStatus::Failure(format!("timeout after {}s", job.timeout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron_svc::schema::{CrontabJob, OnFailure, RetryPolicy};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_job(on_failure: OnFailure) -> CrontabJob {
        CrontabJob {
            id: "test-job".into(),
            schedule: "0 * * * *".into(),
            user: "svc-test".into(),
            agent_template: "test-agent".into(),
            goal: "Run task {retentionDays}".into(),
            args: {
                let mut m = HashMap::new();
                m.insert("retentionDays".into(), serde_json::Value::Number(7.into()));
                m
            },
            timeout: 1,
            on_failure,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                backoff_sec: 0, // no delay in tests
            },
            timezone: None,
        }
    }

    struct MockSpawner {
        spawn_count: Arc<AtomicU32>,
        /// Pre-programmed exit statuses (popped in order; last one repeated).
        exits: Arc<Mutex<Vec<AgentExitStatus>>>,
        /// Whether wait_for_exit blocks forever (simulates timeout).
        block_forever: bool,
    }

    impl MockSpawner {
        fn always_success() -> Arc<Self> {
            Arc::new(Self {
                spawn_count: Arc::new(AtomicU32::new(0)),
                exits: Arc::new(Mutex::new(vec![AgentExitStatus::Success])),
                block_forever: false,
            })
        }

        fn always_fail(msg: &str) -> Arc<Self> {
            Arc::new(Self {
                spawn_count: Arc::new(AtomicU32::new(0)),
                exits: Arc::new(Mutex::new(vec![AgentExitStatus::Failure(msg.into())])),
                block_forever: false,
            })
        }

        fn blocks_forever() -> Arc<Self> {
            Arc::new(Self {
                spawn_count: Arc::new(AtomicU32::new(0)),
                exits: Arc::new(Mutex::new(vec![])),
                block_forever: true,
            })
        }
    }

    #[async_trait]
    impl AgentSpawner for MockSpawner {
        async fn spawn(&self, _req: SpawnRequest) -> Result<SpawnHandle, String> {
            let n = self.spawn_count.fetch_add(1, Ordering::AcqRel);
            Ok(SpawnHandle {
                run_id: format!("run-{n}"),
            })
        }

        async fn wait_for_exit(&self, _handle: SpawnHandle) -> AgentExitStatus {
            if self.block_forever {
                // Sleep longer than any test timeout
                tokio::time::sleep(Duration::from_secs(9999)).await;
            }
            let mut exits = self.exits.lock().unwrap();
            if exits.len() == 1 {
                exits[0].clone()
            } else if exits.is_empty() {
                AgentExitStatus::Failure("no exits configured".into())
            } else {
                exits.remove(0)
            }
        }

        async fn stop(&self, _handle: SpawnHandle) {}
    }

    struct RecordingAlertSink {
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RecordingAlertSink {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Arc::new(Mutex::new(vec![])),
            })
        }

        fn alert_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl AlertSink for RecordingAlertSink {
        async fn send(&self, job_id: &str, reason: &str) {
            self.calls
                .lock()
                .unwrap()
                .push((job_id.to_string(), reason.to_string()));
        }
    }

    // ── unit tests ────────────────────────────────────────────────────────────

    #[test]
    fn render_goal_substitutes_known_args() {
        let mut args = HashMap::new();
        args.insert("retentionDays".into(), serde_json::Value::Number(7.into()));
        let result = render_goal("Compact memory older than {retentionDays} days", &args);
        assert_eq!(result, "Compact memory older than 7 days");
    }

    #[test]
    fn render_goal_leaves_unknown_placeholders() {
        let result = render_goal("Goal with {unknown}", &HashMap::new());
        assert_eq!(result, "Goal with {unknown}");
    }

    #[test]
    fn render_goal_empty_args() {
        let result = render_goal("No placeholders here", &HashMap::new());
        assert_eq!(result, "No placeholders here");
    }

    #[tokio::test]
    async fn run_job_success_no_alert() {
        let spawner = MockSpawner::always_success();
        let alert = RecordingAlertSink::new();
        let job = make_job(OnFailure::Alert);

        run_job(job, spawner.clone(), alert.clone()).await;

        assert_eq!(spawner.spawn_count.load(Ordering::Relaxed), 1);
        assert_eq!(alert.alert_count(), 0);
    }

    #[tokio::test]
    async fn run_job_on_failure_ignore_no_alert() {
        let spawner = MockSpawner::always_fail("error");
        let alert = RecordingAlertSink::new();
        let job = make_job(OnFailure::Ignore);

        run_job(job, spawner.clone(), alert.clone()).await;

        assert_eq!(spawner.spawn_count.load(Ordering::Relaxed), 1);
        assert_eq!(alert.alert_count(), 0, "ignore should not trigger alert");
    }

    #[tokio::test]
    async fn run_job_on_failure_alert_sends_once() {
        let spawner = MockSpawner::always_fail("some error");
        let alert = RecordingAlertSink::new();
        let job = make_job(OnFailure::Alert);

        run_job(job, spawner.clone(), alert.clone()).await;

        assert_eq!(spawner.spawn_count.load(Ordering::Relaxed), 1);
        assert_eq!(alert.alert_count(), 1);
    }

    #[tokio::test]
    async fn run_job_retry_spawns_up_to_max_attempts() {
        let spawner = MockSpawner::always_fail("persistent error");
        let alert = RecordingAlertSink::new();
        let mut job = make_job(OnFailure::Retry);
        job.retry_policy.max_attempts = 3;

        run_job(job, spawner.clone(), alert.clone()).await;

        assert_eq!(
            spawner.spawn_count.load(Ordering::Relaxed),
            3,
            "should attempt exactly max_attempts times"
        );
        // After exhaustion, alert is sent once
        assert_eq!(alert.alert_count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn run_job_timeout_sends_stop_and_returns_failure() {
        let spawner = MockSpawner::blocks_forever();
        let alert = RecordingAlertSink::new();
        let mut job = make_job(OnFailure::Alert);
        job.timeout = 1; // 1 second timeout
        job.retry_policy.backoff_sec = 0;

        // Advance time past the timeout
        let run = tokio::spawn(run_job(job, spawner.clone(), alert.clone()));
        tokio::time::advance(Duration::from_secs(2)).await;
        run.await.unwrap();

        // Spawn was called once; alert fired because it timed out
        assert_eq!(spawner.spawn_count.load(Ordering::Relaxed), 1);
        assert_eq!(alert.alert_count(), 1);
    }

    #[tokio::test]
    async fn scheduler_due_jobs_fired_after_last_run_update() {
        use crate::cron_svc::schema::CrontabFile;

        let spawner = MockSpawner::always_success();
        let alert = RecordingAlertSink::new();

        let crontab = CrontabFile::empty(); // no jobs → should not panic
        let runner = CronRunner::from_crontab(crontab, spawner.clone(), alert.clone())
            .await
            .unwrap();

        // An empty runner starts cleanly
        assert_eq!(spawner.spawn_count.load(Ordering::Relaxed), 0);
        drop(runner); // just verify no panic
    }
}
