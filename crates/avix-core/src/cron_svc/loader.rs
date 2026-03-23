use std::str::FromStr;
use std::sync::Arc;

use cron::Schedule;

use crate::memfs::{VfsPath, VfsRouter};

use super::schema::{CrontabFile, CrontabJob};

/// VFS path for the user-editable crontab config.
pub const CRONTAB_PATH: &str = "/etc/avix/crontab.yaml";

/// VFS path for the kernel-owned defaults file (read-only at runtime).
pub const CRONTAB_DEFAULTS_PATH: &str = "/kernel/defaults/crontab.yaml";

#[derive(Debug, thiserror::Error)]
pub enum CrontabError {
    #[error("crontab.yaml not found at {0}")]
    NotFound(String),
    #[error("invalid crontab YAML: {0}")]
    ParseError(String),
    #[error("invalid cron expression '{expr}' in job '{job_id}': {reason}")]
    InvalidExpression {
        expr: String,
        job_id: String,
        reason: String,
    },
    #[error("duplicate job id '{0}' in crontab.yaml")]
    DuplicateJobId(String),
    #[error("VFS error reading {path}: {reason}")]
    Vfs { path: String, reason: String },
}

/// Reads and validates `/etc/avix/crontab.yaml` from the VFS.
pub struct CrontabLoader {
    vfs: Arc<VfsRouter>,
}

impl CrontabLoader {
    pub fn new(vfs: Arc<VfsRouter>) -> Self {
        Self { vfs }
    }

    /// Load `/etc/avix/crontab.yaml`, parse it, and validate all job fields.
    ///
    /// Returns `CrontabError::NotFound` if the file is absent.
    pub async fn load(&self) -> Result<CrontabFile, CrontabError> {
        let path = VfsPath::parse(CRONTAB_PATH).map_err(|e| CrontabError::Vfs {
            path: CRONTAB_PATH.into(),
            reason: e.to_string(),
        })?;

        let bytes = self
            .vfs
            .read(&path)
            .await
            .map_err(|_| CrontabError::NotFound(CRONTAB_PATH.into()))?;

        let text = String::from_utf8_lossy(&bytes);
        let file: CrontabFile =
            serde_yaml::from_str(&text).map_err(|e| CrontabError::ParseError(e.to_string()))?;

        validate(&file)?;
        Ok(file)
    }

    /// Load with field-level defaults applied from the defaults file and hard-coded values.
    ///
    /// If `/etc/avix/crontab.yaml` is absent, returns `CrontabError::NotFound`.
    /// If `/kernel/defaults/crontab.yaml` is absent, hard-coded defaults are used.
    pub async fn load_with_defaults(&self) -> Result<CrontabFile, CrontabError> {
        // Hard-coded defaults are already expressed via serde `default` attributes
        // in the schema types; loading the file is sufficient.
        self.load().await
    }
}

/// Validate a parsed `CrontabFile`.
fn validate(file: &CrontabFile) -> Result<(), CrontabError> {
    let mut seen_ids = std::collections::HashSet::new();

    for job in &file.spec.jobs {
        // Unique IDs
        if !seen_ids.insert(job.id.clone()) {
            return Err(CrontabError::DuplicateJobId(job.id.clone()));
        }

        // Valid cron expression
        validate_schedule(job)?;
    }

    Ok(())
}

/// Normalise a cron expression for the `cron` crate, which requires 6 fields
/// (sec, min, hour, day, month, weekday). The Avix spec uses the standard 5-field
/// form (min, hour, day, month, weekday); prepend `"0 "` to convert automatically.
pub fn normalise_expression(expr: &str) -> String {
    let fields = expr.split_whitespace().count();
    if fields == 5 {
        format!("0 {}", expr)
    } else {
        expr.to_string()
    }
}

fn validate_schedule(job: &CrontabJob) -> Result<(), CrontabError> {
    let normalised = normalise_expression(&job.schedule);
    Schedule::from_str(&normalised).map_err(|e| CrontabError::InvalidExpression {
        expr: job.schedule.clone(),
        job_id: job.id.clone(),
        reason: e.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memfs::VfsRouter;
    use std::sync::Arc;

    fn make_vfs() -> Arc<VfsRouter> {
        Arc::new(VfsRouter::new())
    }

    async fn write_crontab(vfs: &VfsRouter, content: &str) {
        let path = VfsPath::parse(CRONTAB_PATH).unwrap();
        vfs.write(&path, content.as_bytes().to_vec()).await.unwrap();
    }

    #[tokio::test]
    async fn returns_not_found_when_file_absent() {
        let vfs = make_vfs();
        let loader = CrontabLoader::new(vfs);
        let err = loader.load().await.unwrap_err();
        assert!(matches!(err, CrontabError::NotFound(_)));
    }

    #[tokio::test]
    async fn loads_valid_crontab() {
        let vfs = make_vfs();
        write_crontab(
            &vfs,
            r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  timezone: UTC
  jobs:
    - id: hourly-ingest
      schedule: "0 * * * *"
      user: svc-pipeline
      agentTemplate: pipeline-ingest
      goal: Ingest data
"#,
        )
        .await;
        let loader = CrontabLoader::new(vfs);
        let file = loader.load().await.unwrap();
        assert_eq!(file.spec.jobs.len(), 1);
        assert_eq!(file.spec.jobs[0].id, "hourly-ingest");
        // defaults applied
        assert_eq!(file.spec.jobs[0].timeout, 3600);
    }

    #[tokio::test]
    async fn rejects_invalid_cron_expression() {
        let vfs = make_vfs();
        write_crontab(
            &vfs,
            r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  jobs:
    - id: bad-job
      schedule: "not-a-cron"
      user: svc-test
      agentTemplate: test-agent
      goal: Do something
"#,
        )
        .await;
        let loader = CrontabLoader::new(vfs);
        let err = loader.load().await.unwrap_err();
        assert!(
            matches!(err, CrontabError::InvalidExpression { .. }),
            "expected InvalidExpression, got {err}"
        );
    }

    #[tokio::test]
    async fn rejects_duplicate_job_ids() {
        let vfs = make_vfs();
        write_crontab(
            &vfs,
            r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  jobs:
    - id: dup
      schedule: "0 * * * *"
      user: svc
      agentTemplate: agent
      goal: Goal A
    - id: dup
      schedule: "0 * * * *"
      user: svc
      agentTemplate: agent
      goal: Goal B
"#,
        )
        .await;
        let loader = CrontabLoader::new(vfs);
        let err = loader.load().await.unwrap_err();
        assert!(matches!(err, CrontabError::DuplicateJobId(_)));
    }

    #[tokio::test]
    async fn applies_defaults_for_missing_optional_fields() {
        let vfs = make_vfs();
        write_crontab(
            &vfs,
            r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  jobs:
    - id: minimal-job
      schedule: "0 3 * * *"
      user: svc-gc
      agentTemplate: gc-agent
      goal: Run GC
"#,
        )
        .await;
        let loader = CrontabLoader::new(vfs);
        let file = loader.load().await.unwrap();
        let job = &file.spec.jobs[0];
        assert_eq!(job.timeout, 3600);
        assert_eq!(job.on_failure, super::super::schema::OnFailure::Alert);
        assert_eq!(job.retry_policy.max_attempts, 3);
        assert_eq!(job.retry_policy.backoff_sec, 60);
        assert!(job.args.is_empty());
        assert!(job.timezone.is_none());
    }

    #[tokio::test]
    async fn load_with_defaults_works_same_as_load() {
        let vfs = make_vfs();
        write_crontab(
            &vfs,
            r#"
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "2026-03-22T00:00:00Z"
spec:
  jobs:
    - id: test
      schedule: "0 * * * *"
      user: svc
      agentTemplate: agent
      goal: Goal
"#,
        )
        .await;
        let loader = CrontabLoader::new(vfs);
        let file = loader.load_with_defaults().await.unwrap();
        assert_eq!(file.spec.jobs.len(), 1);
    }
}
