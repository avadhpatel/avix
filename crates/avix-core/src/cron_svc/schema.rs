use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What to do when a cron job exits non-zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    Ignore,
    #[default]
    Alert,
    Retry,
}

/// Retry behaviour when `on_failure == OnFailure::Retry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    /// Maximum number of attempts (initial run + retries). Default: 3.
    #[serde(default = "RetryPolicy::default_max_attempts")]
    pub max_attempts: u32,
    /// Seconds to wait between retry attempts. Default: 60.
    #[serde(default = "RetryPolicy::default_backoff_sec")]
    pub backoff_sec: u64,
}

impl RetryPolicy {
    fn default_max_attempts() -> u32 {
        3
    }
    fn default_backoff_sec() -> u64 {
        60
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: Self::default_max_attempts(),
            backoff_sec: Self::default_backoff_sec(),
        }
    }
}

/// A single scheduled job entry inside a `CrontabFile`.
///
/// Matches the `spec.jobs[]` schema from `docs/spec/crontab.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabJob {
    /// Unique job identifier; used in logs and alerts.
    pub id: String,

    /// Standard 5-field cron expression (UTC unless `timezone` is set).
    pub schedule: String,

    /// Username under whose quota and tool permissions the agent runs.
    pub user: String,

    /// `metadata.name` of the AgentManifest to spawn.
    pub agent_template: String,

    /// Goal string passed to the spawned agent.
    pub goal: String,

    /// Key-value pairs merged into the agent's goal template vars.
    #[serde(default)]
    pub args: HashMap<String, serde_json::Value>,

    /// Max wall-clock seconds. Kernel sends SIGSTOP if exceeded. Default: 3600.
    #[serde(default = "CrontabJob::default_timeout")]
    pub timeout: u64,

    /// What to do when the job exits non-zero. Default: `Alert`.
    #[serde(default)]
    pub on_failure: OnFailure,

    /// Retry policy. Required when `on_failure == OnFailure::Retry`;
    /// defaults are applied when the field is absent.
    #[serde(default)]
    pub retry_policy: RetryPolicy,

    /// Per-job timezone override. `None` inherits `spec.timezone`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl CrontabJob {
    fn default_timeout() -> u64 {
        3600
    }
}

/// Top-level `spec` block of a `CrontabFile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabSpec {
    /// Default timezone for all jobs. Default: `"UTC"`.
    #[serde(default = "CrontabSpec::default_timezone")]
    pub timezone: String,

    pub jobs: Vec<CrontabJob>,
}

impl CrontabSpec {
    fn default_timezone() -> String {
        "UTC".into()
    }
}

impl Default for CrontabSpec {
    fn default() -> Self {
        Self {
            timezone: Self::default_timezone(),
            jobs: Vec::new(),
        }
    }
}

/// Metadata block of a `CrontabFile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabMetadata {
    pub last_updated: DateTime<Utc>,
}

/// Top-level document type for `/etc/avix/crontab.yaml`.
///
/// Matches `apiVersion: avix/v1 / kind: Crontab`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabFile {
    pub api_version: String,
    pub kind: String,
    pub metadata: CrontabMetadata,
    pub spec: CrontabSpec,
}

impl CrontabFile {
    /// An empty crontab (no jobs) — used when the file is absent at boot.
    pub fn empty() -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "Crontab".into(),
            metadata: CrontabMetadata {
                last_updated: Utc::now(),
            },
            spec: CrontabSpec::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_failure_serializes_all_variants() {
        assert_eq!(
            serde_yaml::to_string(&OnFailure::Ignore).unwrap().trim(),
            "ignore"
        );
        assert_eq!(
            serde_yaml::to_string(&OnFailure::Alert).unwrap().trim(),
            "alert"
        );
        assert_eq!(
            serde_yaml::to_string(&OnFailure::Retry).unwrap().trim(),
            "retry"
        );
    }

    #[test]
    fn on_failure_default_is_alert() {
        assert_eq!(OnFailure::default(), OnFailure::Alert);
    }

    #[test]
    fn retry_policy_defaults_when_empty() {
        let p: RetryPolicy = serde_yaml::from_str("{}").unwrap();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.backoff_sec, 60);
    }

    #[test]
    fn crontab_job_timeout_default_is_3600() {
        let yaml = r#"
id: test-job
schedule: "0 * * * *"
user: svc-test
agentTemplate: test-agent
goal: Do something
"#;
        let job: CrontabJob = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(job.timeout, 3600);
        assert_eq!(job.on_failure, OnFailure::Alert);
    }

    #[test]
    fn crontab_job_args_default_empty() {
        let yaml = r#"
id: job1
schedule: "0 * * * *"
user: svc
agentTemplate: agent
goal: Goal
"#;
        let job: CrontabJob = serde_yaml::from_str(yaml).unwrap();
        assert!(job.args.is_empty());
    }

    #[test]
    fn crontab_file_round_trips_yaml() {
        let yaml = r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  timezone: UTC
  jobs:
    - id: hourly-job
      schedule: "0 * * * *"
      user: svc-pipeline
      agentTemplate: pipeline-ingest
      goal: Ingest latest data
      timeout: 1800
      onFailure: retry
      retryPolicy:
        maxAttempts: 3
        backoffSec: 60
"#;
        let file: CrontabFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(file.spec.jobs.len(), 1);
        assert_eq!(file.spec.jobs[0].id, "hourly-job");
        assert_eq!(file.spec.jobs[0].timeout, 1800);
        assert_eq!(file.spec.jobs[0].on_failure, OnFailure::Retry);
        assert_eq!(file.spec.jobs[0].retry_policy.max_attempts, 3);
        assert_eq!(file.spec.timezone, "UTC");
    }

    #[test]
    fn crontab_file_empty_constructor() {
        let f = CrontabFile::empty();
        assert_eq!(f.api_version, "avix/v1");
        assert_eq!(f.kind, "Crontab");
        assert!(f.spec.jobs.is_empty());
    }

    #[test]
    fn crontab_spec_timezone_default_is_utc() {
        let spec: CrontabSpec = serde_yaml::from_str("jobs: []").unwrap();
        assert_eq!(spec.timezone, "UTC");
    }
}
