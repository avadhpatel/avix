# IPC Gap D — Jobs Service (`jobs.svc`)

> **Status:** Not started
> **Priority:** High — required for any long-running tool (LLM inference, file ops, external APIs)
> **Affects:** new `avix-core/src/jobs/`, `avix-core/src/executor/runtime_executor.rs`

---

## Problem

The spec (`ipc-protocol.md §9`) defines a full job lifecycle for tools that take more than a few seconds:
- Tools return `{ "job_id": "...", "status": "running" }` immediately
- Background workers emit `jobs.emit` (progress), `jobs.complete`, and `jobs.fail` notifications
- Callers use `job/watch` tool to observe events

There is currently **no jobs subsystem at all** in the codebase:
- No `jobs/` module
- No `job/watch` implementation (it is listed as an always-present Cat2 tool but has no handler)
- No `job_id` generation or tracking
- No progress event bus
- Tools are all synchronous; no mechanism to return a job ID and continue in the background

Without this, `llm.svc` inference calls (which are long-running) cannot be properly modeled, and any tool that takes more than a few seconds blocks the IPC connection.

---

## What Needs to Be Built

### 1. Job State Machine (`jobs/job.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Pending,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,                          // "job-<ulid>"
    pub tool: String,                        // tool that created this job
    pub owner_pid: Pid,
    pub state: JobState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub result: Option<serde_json::Value>,   // set on Done
    pub error: Option<JobError>,             // set on Failed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    pub code: i32,
    pub message: String,
}
```

State transitions (per spec §9):

```
pending → running → done
                  → failed
running → paused → running
                → failed (on CANCEL)
```

### 2. Job Event Types (`jobs/event.rs`)

```rust
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
        percent: Option<u8>,
        stage: Option<String>,
        detail: Option<String>,
    },
    Log {
        job_id: String,
        stream: LogStream,   // Stdout | Stderr
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream { Stdout, Stderr }
```

### 3. Job Registry (`jobs/registry.rs`)

```rust
pub struct JobRegistry {
    jobs: HashMap<String, Job>,
    events: broadcast::Sender<JobEvent>,
}

impl JobRegistry {
    pub fn new() -> (Self, broadcast::Receiver<JobEvent>);

    /// Create a new job record. Returns the job_id.
    pub fn create(&mut self, tool: &str, owner_pid: Pid) -> String;

    /// Transition a job to Running state.
    pub fn start(&mut self, job_id: &str) -> Result<(), AvixError>;

    /// Emit a progress event. Does not change state.
    pub fn progress(&mut self, job_id: &str, percent: Option<u8>, stage: Option<String>, detail: Option<String>) -> Result<(), AvixError>;

    /// Emit a log line event.
    pub fn log(&mut self, job_id: &str, stream: LogStream, line: String) -> Result<(), AvixError>;

    /// Transition to Done and emit Complete event.
    pub fn complete(&mut self, job_id: &str, result: serde_json::Value) -> Result<(), AvixError>;

    /// Transition to Failed and emit Fail event.
    pub fn fail(&mut self, job_id: &str, error: JobError) -> Result<(), AvixError>;

    /// Cancel a running or paused job.
    pub fn cancel(&mut self, job_id: &str) -> Result<(), AvixError>;

    /// Get current job state.
    pub fn get(&self, job_id: &str) -> Result<&Job, AvixError>;

    /// Subscribe to all future events.
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent>;

    /// Subscribe to events for a specific job_id only.
    /// Returns a channel that auto-closes when the job reaches a terminal state.
    pub fn watch(&self, job_id: &str) -> Result<broadcast::Receiver<JobEvent>, AvixError>;

    pub fn job_count(&self) -> usize;
}
```

### 4. `job/watch` Tool Handler (`jobs/watch_handler.rs`)

This is the always-present Cat2 tool. Receives a `job_id`, subscribes to events, and streams them back. Since IPC is request-response (not streaming), the design is:

- **Polling model:** `job/watch` takes `{ "job_id": "...", "timeout_ms": 5000 }` and returns the next event within the timeout, or `{ "status": "timeout" }` if none arrives
- Agent calls `job/watch` in a loop until it receives `Complete` or `Fail`

```rust
pub async fn handle_job_watch(
    job_id: String,
    timeout_ms: u64,
    registry: Arc<RwLock<JobRegistry>>,
) -> Result<serde_json::Value, AvixError>;
```

Returns one of:
- `{ "event": <JobEvent as JSON> }` — the next event for this job
- `{ "status": "timeout" }` — no event within timeout
- `{ "status": "not_found" }` — job_id unknown

### 5. `jobs.emit`, `jobs.complete`, `jobs.fail` IPC Handlers

These are called by services (not agents) to push events into the registry. They are received by the router on `AVIX_ROUTER_SOCK` as notifications (no `id`).

```rust
pub async fn handle_jobs_emit(
    token: String,
    job_id: String,
    event: JobEvent,
    registry: Arc<RwLock<JobRegistry>>,
    service_manager: &ServiceManager,
) -> Result<(), AvixError>;

pub async fn handle_jobs_complete(
    token: String,
    job_id: String,
    result: serde_json::Value,
    registry: Arc<RwLock<JobRegistry>>,
    service_manager: &ServiceManager,
) -> Result<(), AvixError>;

pub async fn handle_jobs_fail(
    token: String,
    job_id: String,
    error: JobError,
    registry: Arc<RwLock<JobRegistry>>,
    service_manager: &ServiceManager,
) -> Result<(), AvixError>;
```

All verify that `token` belongs to the service that owns the job before accepting events (prevent rogue services from completing other services' jobs).

### 6. Job-Based Tool Invocation Helper

Provide a helper for service authors:

```rust
/// Start a job-style tool call: validate, create job record, spawn background task.
/// Returns job_id immediately (service should return this to IPC caller).
pub async fn start_job<F, Fut>(
    tool: &str,
    owner_pid: Pid,
    registry: Arc<RwLock<JobRegistry>>,
    work: F,
) -> String
where
    F: FnOnce(String, Arc<RwLock<JobRegistry>>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static;
```

The `work` closure receives `(job_id, registry_ref)` and is responsible for calling `registry.progress()`, `registry.complete()`, or `registry.fail()`.

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/jobs.rs`.

```rust
// T-D-01: Create job starts in Pending state
#[tokio::test]
async fn create_job_is_pending() {
    let (mut reg, _) = JobRegistry::new();
    let id = reg.create("fs/write", Pid::from(10));
    assert_eq!(reg.get(&id).unwrap().state, JobState::Pending);
}

// T-D-02: start() transitions Pending → Running
#[tokio::test]
async fn start_job_transitions_to_running() {
    let (mut reg, mut rx) = JobRegistry::new();
    let id = reg.create("fs/write", Pid::from(10));
    reg.start(&id).unwrap();
    assert_eq!(reg.get(&id).unwrap().state, JobState::Running);
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, JobEvent::StatusChange { new_state: JobState::Running, .. }));
}

// T-D-03: progress() emits Progress event without state change
#[tokio::test]
async fn progress_event_emitted() {
    // create, start job
    // call progress(50, Some("encoding"), None)
    // assert Progress event received on subscriber
    // assert state still Running
}

// T-D-04: complete() transitions Running → Done
#[tokio::test]
async fn complete_job_transitions_to_done() {
    // create, start, complete
    // assert state == Done
    // assert Complete event emitted with result value
}

// T-D-05: fail() transitions Running → Failed
#[tokio::test]
async fn fail_job_transitions_to_failed() {
    // create, start, fail
    // assert state == Failed
    // assert Fail event emitted with error
}

// T-D-06: Cannot complete a Done job
#[tokio::test]
async fn complete_done_job_returns_error() {
    // create, start, complete
    // call complete again
    // assert Err (Econflict)
}

// T-D-07: cancel() transitions Running → Cancelled
#[tokio::test]
async fn cancel_running_job() {
    // create, start, cancel
    // assert state == Cancelled
}

// T-D-08: job/watch returns next event within timeout
#[tokio::test]
async fn job_watch_returns_next_event() {
    // create, start job in background
    // spawn task that emits progress after 10ms
    // call handle_job_watch(job_id, 500ms)
    // assert Progress event returned
}

// T-D-09: job/watch times out if no event
#[tokio::test]
async fn job_watch_times_out() {
    // create, start job (no progress emitted)
    // call handle_job_watch(job_id, 50ms)
    // assert { "status": "timeout" }
}

// T-D-10: job/watch returns not_found for unknown job
#[tokio::test]
async fn job_watch_unknown_job() {
    // call handle_job_watch("ghost-job-id", 100ms)
    // assert { "status": "not_found" }
}

// T-D-11: start_job helper runs background work
#[tokio::test]
async fn start_job_helper_runs_to_completion() {
    // call start_job("fs/write", pid, registry, |job_id, reg| async {
    //   tokio::time::sleep(10ms)
    //   reg.write().complete(job_id, json!({"bytes": 42}))
    // })
    // wait for Complete event
    // assert job state == Done
}

// T-D-12: Concurrent jobs tracked independently
#[tokio::test]
async fn concurrent_jobs_are_independent() {
    // create and run 5 jobs concurrently
    // complete them in different orders
    // assert each reaches Done with correct result
}
```

---

## Implementation Notes

- Job IDs: use `ulid` crate (`job-<ULID>`) for sortable, unique IDs with embedded timestamps
- `broadcast::Sender` capacity for job events: 256 (more than signal bus — events can pile up for slow watchers)
- `watch()` returns a filtered receiver — use `ReceiverStream` + `filter` from `tokio_stream` to filter by `job_id`
- Alternatively, keep it simple: `watch()` returns the global event receiver; `handle_job_watch` filters events by `job_id` internally
- Job registry is in-memory only (no persistence for now)
- Orphaned jobs (owner process died): periodically scan for jobs owned by dead PIDs; transition to `Failed` with message "owner process exited". Wire this up when the process table has a cleanup hook.
- Do not add `jobs.svc` as a separate process yet — embed `JobRegistry` in the kernel's service manager for this phase

---

## Success Criteria

- [ ] `Job`, `JobState`, `JobEvent` types defined
- [ ] `JobRegistry` with full lifecycle methods implemented
- [ ] `job/watch` handler implemented (polling model, timeout)
- [ ] `jobs.emit`, `jobs.complete`, `jobs.fail` handlers implemented
- [ ] `start_job` helper implemented
- [ ] All T-D-* tests pass
- [ ] `job/watch` wired into `RuntimeExecutor`'s always-present tool dispatch
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes (no regressions)
