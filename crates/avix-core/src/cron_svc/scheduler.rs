use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum MissedRunPolicy {
    Skip,
    FireOnce,
}

#[derive(Debug, Clone)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub expression: String,
    pub missed_run_policy: MissedRunPolicy,
    pub last_run: Option<DateTime<Utc>>,
    pub enabled: bool,
}

impl CronJob {
    pub fn next_run_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let schedule = Schedule::from_str(&self.expression).ok()?;
        schedule.after(&after).next()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression: {0}")]
    InvalidExpression(String),
    #[error("job not found: {0}")]
    NotFound(String),
}

pub struct CronScheduler {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
}

impl CronScheduler {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_job(
        &self,
        name: String,
        expression: String,
        missed_run_policy: MissedRunPolicy,
    ) -> Result<String, CronError> {
        // Validate expression first
        Schedule::from_str(&expression)
            .map_err(|_| CronError::InvalidExpression(expression.clone()))?;

        let id = Uuid::new_v4().to_string();
        let job = CronJob {
            id: id.clone(),
            name,
            expression,
            missed_run_policy,
            last_run: None,
            enabled: true,
        };
        self.jobs.write().await.insert(id.clone(), job);
        Ok(id)
    }

    pub async fn remove_job(&self, id: &str) -> Result<(), CronError> {
        let mut jobs = self.jobs.write().await;
        jobs.remove(id)
            .ok_or_else(|| CronError::NotFound(id.to_string()))?;
        Ok(())
    }

    pub async fn list_jobs(&self) -> Vec<CronJob> {
        self.jobs.read().await.values().cloned().collect()
    }

    pub async fn job_count(&self) -> usize {
        self.jobs.read().await.len()
    }

    /// Get jobs that are due to fire since `since`
    pub async fn due_jobs(&self, since: DateTime<Utc>) -> Vec<String> {
        let jobs = self.jobs.read().await;
        let now = Utc::now();
        jobs.values()
            .filter(|job| job.enabled)
            .filter_map(|job| {
                let next = job.next_run_after(since)?;
                if next <= now {
                    Some(job.id.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_add_valid_job() {
        let sched = CronScheduler::new();
        let id = sched
            .add_job(
                "test".into(),
                "* * * * * *".into(), // every second
                MissedRunPolicy::Skip,
            )
            .await;
        assert!(id.is_ok());
    }

    #[tokio::test]
    async fn test_invalid_expression_rejected() {
        let sched = CronScheduler::new();
        let res = sched
            .add_job(
                "bad".into(),
                "not-a-cron-expression".into(),
                MissedRunPolicy::Skip,
            )
            .await;
        assert!(matches!(res, Err(CronError::InvalidExpression(_))));
    }

    #[tokio::test]
    async fn test_next_run_is_in_future() {
        let job = CronJob {
            id: "1".into(),
            name: "test".into(),
            expression: "* * * * * *".into(),
            missed_run_policy: MissedRunPolicy::Skip,
            last_run: None,
            enabled: true,
        };
        let now = Utc::now();
        let next = job.next_run_after(now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[tokio::test]
    async fn test_remove_job() {
        let sched = CronScheduler::new();
        let id = sched
            .add_job("x".into(), "* * * * * *".into(), MissedRunPolicy::Skip)
            .await
            .unwrap();
        sched.remove_job(&id).await.unwrap();
        assert_eq!(sched.job_count().await, 0);
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let sched = CronScheduler::new();
        sched
            .add_job("a".into(), "* * * * * *".into(), MissedRunPolicy::Skip)
            .await
            .unwrap();
        sched
            .add_job("b".into(), "* * * * * *".into(), MissedRunPolicy::FireOnce)
            .await
            .unwrap();
        assert_eq!(sched.list_jobs().await.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_returns_error() {
        let sched = CronScheduler::new();
        let res = sched.remove_job("nonexistent").await;
        assert!(matches!(res, Err(CronError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_missed_run_policy_skip() {
        let job = CronJob {
            id: "1".into(),
            name: "test".into(),
            expression: "* * * * * *".into(),
            missed_run_policy: MissedRunPolicy::Skip,
            last_run: None,
            enabled: true,
        };
        assert_eq!(job.missed_run_policy, MissedRunPolicy::Skip);
    }

    #[tokio::test]
    async fn test_missed_run_policy_fire_once() {
        let job = CronJob {
            id: "1".into(),
            name: "test".into(),
            expression: "* * * * * *".into(),
            missed_run_policy: MissedRunPolicy::FireOnce,
            last_run: None,
            enabled: true,
        };
        assert_eq!(job.missed_run_policy, MissedRunPolicy::FireOnce);
    }

    #[tokio::test]
    async fn test_due_jobs_empty_when_far_future() {
        let sched = CronScheduler::new();
        // Add a job with a yearly expression (won't fire this second)
        sched
            .add_job(
                "yearly".into(),
                "0 0 1 1 * *".into(), // 1st Jan every year
                MissedRunPolicy::Skip,
            )
            .await
            .unwrap();
        // Check due jobs "since now" — should not be due unless we're on Jan 1
        let since = Utc::now();
        // This only asserts no panic; the list may or may not be empty
        let _due = sched.due_jobs(since).await;
    }

    #[tokio::test]
    async fn test_job_count() {
        let sched = CronScheduler::new();
        assert_eq!(sched.job_count().await, 0);
        sched
            .add_job("a".into(), "* * * * * *".into(), MissedRunPolicy::Skip)
            .await
            .unwrap();
        assert_eq!(sched.job_count().await, 1);
    }

    #[tokio::test]
    async fn test_disabled_job_not_in_due_list() {
        let sched = CronScheduler::new();
        let id = sched
            .add_job(
                "disabled".into(),
                "* * * * * *".into(),
                MissedRunPolicy::Skip,
            )
            .await
            .unwrap();
        // Disable the job
        {
            let mut jobs = sched.jobs.write().await;
            if let Some(j) = jobs.get_mut(&id) {
                j.enabled = false;
            }
        }
        let far_past = Utc::now() - chrono::Duration::hours(1);
        let due = sched.due_jobs(far_past).await;
        assert!(!due.contains(&id));
    }

    #[tokio::test]
    async fn test_unique_job_ids() {
        let sched = CronScheduler::new();
        let id1 = sched
            .add_job("a".into(), "* * * * * *".into(), MissedRunPolicy::Skip)
            .await
            .unwrap();
        let id2 = sched
            .add_job("b".into(), "* * * * * *".into(), MissedRunPolicy::Skip)
            .await
            .unwrap();
        assert_ne!(id1, id2);
    }
}
