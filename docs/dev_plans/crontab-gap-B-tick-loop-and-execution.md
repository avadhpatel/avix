# Crontab Gap B — Tick Loop, Agent Spawn & Failure Handling

> **Status:** Not started
> **Priority:** High — completes the cron subsystem end-to-end
> **Depends on:** Crontab Gap A (schema + loader)
> **Affects:**
> - `avix-core/src/cron_svc/scheduler.rs` (extend)
> - `avix-core/src/cron_svc/runner.rs` (new)
> - `avix-core/src/kernel/boot.rs` (wire runner into startup)

---

## Problem

`CronScheduler::due_jobs()` identifies which jobs are overdue, but nothing ever calls it
and nothing ever acts on the result. The cron subsystem has no background tick loop, no
mechanism to spawn agents, no timeout enforcement, and no failure handling. The scheduler
is effectively dead code at runtime.

---

## What Needs to Be Built

### 1. Wire `CrontabLoader` into `CronScheduler`

**File:** `avix-core/src/cron_svc/scheduler.rs`

Add a constructor that initialises from a loaded `CrontabFile`:

```rust
impl CronScheduler {
    /// Build a scheduler from a fully parsed and validated CrontabFile.
    pub fn from_crontab(crontab: CrontabFile) -> Result<Self, CronError>;
}
```

This replaces individual `add_job()` calls at boot. The scheduler internally stores the
full `CronJob` struct (from Gap A) so the runner has all fields available when a job fires.

---

### 2. `CronRunner` — background tick loop

**File:** `avix-core/src/cron_svc/runner.rs` (new)

```rust
pub struct CronRunner {
    scheduler: Arc<RwLock<CronScheduler>>,
    kernel_ipc: Arc<dyn KernelIpc>,   // used to call kernel/proc/spawn
    alert_sink: Arc<dyn AlertSink>,   // used for onFailure: alert
}

impl CronRunner {
    pub fn new(
        scheduler: Arc<RwLock<CronScheduler>>,
        kernel_ipc: Arc<dyn KernelIpc>,
        alert_sink: Arc<dyn AlertSink>,
    ) -> Self;

    /// Starts the background tick loop. Returns a JoinHandle.
    /// Tick interval: 30 seconds (configurable via KernelConfig.cron.tickIntervalSec).
    pub fn start(self) -> tokio::task::JoinHandle<()>;
}
```

**Tick loop logic (per tick):**

```
1. Record tick_time = Utc::now()
2. let due = scheduler.due_jobs(last_tick_time)
3. For each due job:
   a. Spawn a tokio task: run_job(job.clone())
   b. Update job.last_run = tick_time in the scheduler
4. last_tick_time = tick_time
5. Sleep 30s (or configurable interval)
```

**`run_job` task:**

```
1. Call kernel/proc/spawn with:
     agentTemplate: job.agent_template
     user:          job.user
     goal:          job.goal (with job.args interpolated)
     timeout:       job.timeout
2. Wait for agent exit (watch job/<job_id> via jobs.svc OR poll /proc/<pid>/status.yaml)
3. If agent exits cleanly → done
4. If timeout exceeded → send SIGSTOP to the agent's PID → mark as failed
5. On failure, apply job.on_failure:
     ignore → log at debug! and discard
     alert  → call alert_sink.send(job.id, reason)
     retry  → retry up to retryPolicy.maxAttempts times with backoffSec between attempts
              (exponential back-off is not required; flat interval is sufficient)
              After maxAttempts exhausted, fall through to alert behaviour
```

---

### 3. `AlertSink` trait

**File:** `avix-core/src/cron_svc/runner.rs`

```rust
#[async_trait::async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, job_id: &str, reason: &str);
}
```

Provide a `LogAlertSink` default implementation that emits `tracing::warn!`. A real
notification channel (email, Slack) is out of scope for this gap.

---

### 4. Wire `CronRunner` into kernel boot

**File:** `avix-core/src/kernel/boot.rs`

After Phase 2 (services started):

```rust
let crontab = CrontabLoader::new(vfs.clone())
    .load_with_defaults()
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("crontab.yaml missing or invalid — cron disabled: {e}");
        CrontabFile::empty()
    });

let scheduler = Arc::new(RwLock::new(
    CronScheduler::from_crontab(crontab)?
));

let runner = CronRunner::new(scheduler, kernel_ipc.clone(), Arc::new(LogAlertSink));
let _cron_handle = runner.start();
// hold handle in KernelState so it's not dropped
```

If `crontab.yaml` is absent, the runner starts with an empty job list. This is not an
error — fresh installs have no scheduled jobs.

---

### 5. Timeout enforcement

Timeout is enforced inside `run_job`. After spawning the agent, use
`tokio::time::timeout` around the wait loop:

```rust
let result = tokio::time::timeout(
    Duration::from_secs(job.timeout),
    wait_for_agent_exit(pid),
).await;

if result.is_err() {
    tracing::warn!(job_id = %job.id, "job timed out after {}s — sending SIGSTOP", job.timeout);
    kernel_ipc.send_signal(pid, SignalKind::Stop).await?;
    // treat as failure
}
```

---

### 6. Goal template interpolation

`job.goal` may contain `{varName}` placeholders. Substitute from `job.args` before spawn:

```rust
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
```

Unknown placeholders are left as-is (no error). This is intentional — agents can
interpret remaining template syntax themselves.

---

## Test Plan

### Unit Tests — `runner.rs`

```rust
#[test]
fn render_goal_substitutes_args() {
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
```

### Integration Tests

```rust
#[tokio::test]
async fn scheduler_fires_due_job() {
    // Build a scheduler with one job scheduled every minute
    // Set last_run to 2 minutes ago
    // Call due_jobs(last_tick) — assert the job is returned
    // Call run_job with a mock KernelIpc that records spawn calls
    // Assert spawn was called once with correct user/agentTemplate/goal
}

#[tokio::test]
async fn runner_retries_on_failure_up_to_max_attempts() {
    // Mock KernelIpc that always returns a failed agent exit
    // Job: onFailure: retry, retryPolicy: { maxAttempts: 3, backoffSec: 0 }
    // Run run_job once
    // Assert spawn was called 3 times total (1 initial + 2 retries)
    // Assert alert_sink.send was called once after exhausting retries
}

#[tokio::test]
async fn runner_sends_sigstop_on_timeout() {
    // Mock KernelIpc where wait_for_agent_exit never completes
    // Job: timeout: 1 (1 second)
    // Run run_job with tokio::time::pause + advance
    // Assert SIGSTOP was sent to the spawned PID
}

#[tokio::test]
async fn runner_ignores_failure_when_on_failure_is_ignore() {
    // Mock KernelIpc returning failed exit
    // Job: onFailure: ignore
    // Run run_job — assert alert_sink.send was NOT called
}
```

---

## Success Criteria

- [ ] `CronScheduler::from_crontab` builds a scheduler from a `CrontabFile` without error.
- [ ] `CronRunner::start` returns a `JoinHandle` and does not panic on empty job list.
- [ ] Tick loop calls `due_jobs` and fires `run_job` for each overdue job.
- [ ] `run_job` calls `kernel/proc/spawn` with correct `user`, `agentTemplate`, `goal`, `timeout`.
- [ ] `goal` placeholders are substituted from `job.args` before spawn.
- [ ] Timeout exceeded → `SIGSTOP` sent to agent PID.
- [ ] `onFailure: ignore` → no alert emitted.
- [ ] `onFailure: alert` → `AlertSink::send` called once.
- [ ] `onFailure: retry` → spawned up to `maxAttempts` times, then alert on exhaustion.
- [ ] Kernel boot starts `CronRunner`; absent `crontab.yaml` logs a warning and continues cleanly.
- [ ] `cargo test --workspace` passes, `cargo clippy -- -D warnings` is clean.
