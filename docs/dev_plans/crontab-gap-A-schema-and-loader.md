# Crontab Gap A — Full Schema, VFS Loader & Defaults

> **Status:** Not started
> **Priority:** High — prerequisite for cron gap B (tick loop + agent spawn)
> **Depends on:** None (no other gaps required to define types)
> **Affects:**
> - `avix-core/src/cron_svc/schema.rs` (new)
> - `avix-core/src/cron_svc/loader.rs` (new)
> - `avix-core/src/cron_svc/scheduler.rs` (extend existing `CronJob`)
> - `avix-core/src/cli/config_init.rs` (defaults file write)

---

## Problem

`CronScheduler` and `CronJob` exist as an in-memory scheduler, but the struct shape does
not match the spec (`docs/spec/crontab.md`). The current `CronJob` tracks only scheduling
fields (`expression`, `enabled`, `last_run`); it is missing every field that tells the
kernel **what to do** when the job fires:

| Spec field                          | Current state                         |
|-------------------------------------|---------------------------------------|
| `user`                              | Missing                               |
| `agentTemplate`                     | Missing                               |
| `goal`                              | Missing                               |
| `args`                              | Missing                               |
| `timeout`                           | Missing (default: 3600 s)             |
| `onFailure`                         | Missing (default: `alert`)            |
| `retryPolicy.maxAttempts`           | Missing (default: 3)                  |
| `retryPolicy.backoffSec`            | Missing (default: 60 s)               |
| `timezone` (per-job override)       | Missing (global `spec.timezone` only) |

Additionally there is no typed `Crontab` document struct, no loader that reads
`/etc/avix/crontab.yaml` from the VFS, and no `/kernel/defaults/crontab.yaml` written
during `config init`.

---

## What Needs to Be Built

### 1. `OnFailure` and `RetryPolicy` types

**File:** `avix-core/src/cron_svc/schema.rs` (new)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnFailure {
    Ignore,
    #[default]
    Alert,
    Retry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPolicy {
    #[serde(default = "RetryPolicy::default_max_attempts")]
    pub max_attempts: u32,           // default: 3
    #[serde(default = "RetryPolicy::default_backoff_sec")]
    pub backoff_sec: u64,            // default: 60
}

impl RetryPolicy {
    fn default_max_attempts() -> u32 { 3 }
    fn default_backoff_sec() -> u64 { 60 }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: Self::default_max_attempts(),
            backoff_sec: Self::default_backoff_sec(),
        }
    }
}
```

---

### 2. Extend `CronJob` to match spec

**File:** `avix-core/src/cron_svc/schema.rs`

Replace or extend the existing `CronJob` struct. The new struct is the canonical definition
used by both the scheduler and the loader.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
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
    pub args: std::collections::HashMap<String, serde_json::Value>,

    /// Max wall-clock seconds. Kernel sends SIGSTOP if exceeded. Default: 3600.
    #[serde(default = "CronJob::default_timeout")]
    pub timeout: u64,

    /// What to do when the job exits non-zero. Default: Alert.
    #[serde(default)]
    pub on_failure: OnFailure,

    /// Required when `on_failure == OnFailure::Retry`.
    #[serde(default)]
    pub retry_policy: RetryPolicy,

    /// Per-job timezone override. If absent, inherits `spec.timezone`.
    pub timezone: Option<String>,

    // --- scheduler runtime fields (not in YAML, populated by loader/scheduler) ---
    #[serde(skip)]
    pub last_run: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip)]
    pub enabled: bool,
}

impl CronJob {
    fn default_timeout() -> u64 { 3600 }
}
```

> **Note:** The existing `CronScheduler` uses a different `CronJob` shape. After this gap
> lands, replace its internal type with the new schema `CronJob`. Scheduler tests must be
> updated accordingly.

---

### 3. `CrontabSpec` — top-level YAML document

**File:** `avix-core/src/cron_svc/schema.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabSpec {
    /// Default timezone for all jobs. Default: "UTC".
    #[serde(default = "CrontabSpec::default_timezone")]
    pub timezone: String,

    pub jobs: Vec<CronJob>,
}

impl CrontabSpec {
    fn default_timezone() -> String { "UTC".into() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabFile {
    pub api_version: String,   // "avix/v1"
    pub kind: String,          // "Crontab"
    pub metadata: CrontabMetadata,
    pub spec: CrontabSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrontabMetadata {
    pub last_updated: chrono::DateTime<chrono::Utc>,
}
```

---

### 4. `CrontabLoader` — reads from VFS

**File:** `avix-core/src/cron_svc/loader.rs` (new)

```rust
pub const CRONTAB_PATH: &str = "/etc/avix/crontab.yaml";
pub const CRONTAB_DEFAULTS_PATH: &str = "/kernel/defaults/crontab.yaml";

pub struct CrontabLoader {
    vfs: Arc<dyn Vfs>,
}

impl CrontabLoader {
    pub fn new(vfs: Arc<dyn Vfs>) -> Self { Self { vfs } }

    /// Load and parse `/etc/avix/crontab.yaml`.
    /// Returns `CrontabError::NotFound` if the file is absent.
    pub async fn load(&self) -> Result<CrontabFile, CrontabError>;

    /// Merge defaults from `/kernel/defaults/crontab.yaml` into a `CrontabFile`.
    /// Per-job fields already set are not overwritten.
    pub async fn load_with_defaults(&self) -> Result<CrontabFile, CrontabError>;
}

#[derive(Debug, thiserror::Error)]
pub enum CrontabError {
    #[error("crontab.yaml not found at {0}")]
    NotFound(String),
    #[error("invalid crontab YAML: {0}")]
    ParseError(String),
    #[error("invalid cron expression '{expr}' in job '{job_id}': {reason}")]
    InvalidExpression { expr: String, job_id: String, reason: String },
    #[error("retry_policy required when on_failure is retry (job: {0})")]
    MissingRetryPolicy(String),
    #[error("VFS error: {0}")]
    Vfs(String),
}
```

`load_with_defaults` applies field-level defaults in this order (highest wins):
1. Per-job explicit value
2. Global `spec.timezone` for the `timezone` field
3. `/kernel/defaults/crontab.yaml` values
4. Hard-coded Rust defaults

---

### 5. Validation in `CrontabLoader::load`

After parsing, validate:

- Every `schedule` is a valid 5-field cron expression (use `cron::Schedule::from_str`).
- Every `id` is non-empty and unique within the file.
- `on_failure: retry` implies `retry_policy` is present (or default is applied).
- `user` and `agent_template` are non-empty strings.

Return `CrontabError::InvalidExpression` or `CrontabError::MissingRetryPolicy` as
appropriate. Do **not** silently ignore bad expressions.

---

### 6. Write `/kernel/defaults/crontab.yaml` in `config_init`

**File:** `avix-core/src/cli/config_init.rs`

During `avix config init`, write `/kernel/defaults/crontab.yaml` alongside the existing
`/etc/avix/crontab.yaml`:

```yaml
# /kernel/defaults/crontab.yaml
apiVersion: avix/v1
kind: CrontabDefaults
spec:
  timezone: UTC
  jobs:
    timeout: 3600
    onFailure: alert
    retryPolicy:
      maxAttempts: 3
      backoffSec: 60
```

This file is read-only at runtime (lives under `/kernel/`; agents cannot write it).

Also update the existing `/etc/avix/crontab.yaml` template to include `metadata`:

```yaml
apiVersion: avix/v1
kind: Crontab
metadata:
  lastUpdated: "<RFC3339 timestamp of config init>"
spec:
  timezone: UTC
  jobs: []
```

---

## Test Plan

### Unit Tests — `schema.rs`

```rust
#[test]
fn on_failure_serializes_correctly() {
    assert_eq!(serde_yaml::to_string(&OnFailure::Ignore).unwrap().trim(), "ignore");
    assert_eq!(serde_yaml::to_string(&OnFailure::Alert).unwrap().trim(), "alert");
    assert_eq!(serde_yaml::to_string(&OnFailure::Retry).unwrap().trim(), "retry");
}

#[test]
fn retry_policy_defaults() {
    let p: RetryPolicy = serde_yaml::from_str("{}").unwrap();
    assert_eq!(p.max_attempts, 3);
    assert_eq!(p.backoff_sec, 60);
}

#[test]
fn cron_job_timeout_default() {
    let yaml = r#"
        id: test-job
        schedule: "0 * * * *"
        user: svc-test
        agentTemplate: test-agent
        goal: Do something
    "#;
    let job: CronJob = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(job.timeout, 3600);
    assert_eq!(job.on_failure, OnFailure::Alert);
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
    assert_eq!(file.spec.jobs[0].timeout, 1800);
    assert_eq!(file.spec.jobs[0].on_failure, OnFailure::Retry);
}
```

### Unit Tests — `loader.rs`

```rust
#[tokio::test]
async fn loader_returns_not_found_when_file_absent() {
    let vfs = Arc::new(MemFs::new());
    let loader = CrontabLoader::new(vfs);
    let err = loader.load().await.unwrap_err();
    assert!(matches!(err, CrontabError::NotFound(_)));
}

#[tokio::test]
async fn loader_rejects_invalid_cron_expression() {
    let vfs = Arc::new(MemFs::new());
    vfs.write("/etc/avix/crontab.yaml", br#"
        apiVersion: avix/v1
        kind: Crontab
        metadata:
          lastUpdated: "2026-03-22T00:00:00Z"
        spec:
          timezone: UTC
          jobs:
            - id: bad-job
              schedule: "not-a-cron"
              user: svc-test
              agentTemplate: test-agent
              goal: Do something
    "#.to_vec()).await.unwrap();
    let loader = CrontabLoader::new(vfs);
    let err = loader.load().await.unwrap_err();
    assert!(matches!(err, CrontabError::InvalidExpression { .. }));
}

#[tokio::test]
async fn loader_rejects_duplicate_job_ids() {
    // write crontab.yaml with two jobs sharing the same id
    // assert CrontabError returned (define an appropriate variant)
}

#[tokio::test]
async fn loader_applies_defaults_for_missing_fields() {
    // write crontab.yaml with a job that omits timeout and onFailure
    // assert loaded job has timeout == 3600, on_failure == Alert
}
```

---

## Success Criteria

- [ ] `OnFailure` serialises/deserialises all three variants correctly.
- [ ] `RetryPolicy` defaults to `maxAttempts: 3`, `backoffSec: 60` when fields absent.
- [ ] `CronJob.timeout` defaults to `3600` when field absent.
- [ ] `CronJob.on_failure` defaults to `Alert` when field absent.
- [ ] `CrontabFile` round-trips YAML without data loss.
- [ ] `CrontabLoader::load` returns `NotFound` when `/etc/avix/crontab.yaml` is absent.
- [ ] Invalid cron expressions are rejected with `CrontabError::InvalidExpression`.
- [ ] Duplicate job IDs are rejected.
- [ ] `on_failure: retry` without a `retryPolicy` key still works (defaults applied).
- [ ] `config init` writes `/kernel/defaults/crontab.yaml` and an updated `/etc/avix/crontab.yaml` template with `metadata`.
- [ ] `cargo test --workspace` passes, `cargo clippy -- -D warnings` is clean.
