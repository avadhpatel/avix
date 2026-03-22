# Day 24 — Crontab Scheduler

> **Goal:** Implement the cron-style scheduler: parse `crontab.yaml`, evaluate schedules using cron expressions, fire `kernel/proc/spawn` at the scheduled time, handle missed runs (skip policy), and persist next-run state.

---

## Pre-flight: Verify Day 23

```bash
cargo test --workspace
grep -r "SecretsStore"   crates/avix-core/src/
grep -r "vfs_read"       crates/avix-core/src/secrets/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Add Cron Dependency

In `crates/avix-core/Cargo.toml`:

```toml
[dependencies]
cron = "0.12"
```

Add to `src/lib.rs`: `pub mod scheduler;`

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/scheduler.rs`:

```rust
use avix_core::scheduler::{CronScheduler, CronJob, MissedRunPolicy};

// ── Parse cron expression ─────────────────────────────────────────────────────

#[test]
fn parse_valid_cron_expression() {
    let job = CronJob {
        name:               "daily-report".into(),
        schedule:           "0 0 * * *".into(),
        agent:              "reporter".into(),
        goal:               "Generate daily report".into(),
        missed_run_policy:  MissedRunPolicy::Skip,
    };
    assert!(job.parse_schedule().is_ok());
}

#[test]
fn parse_invalid_cron_expression_fails() {
    let job = CronJob {
        name: "bad".into(), schedule: "not-a-cron".into(),
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::Skip,
    };
    assert!(job.parse_schedule().is_err());
}

// ── Next run calculation ──────────────────────────────────────────────────────

#[test]
fn next_run_after_now_is_in_future() {
    let job = CronJob {
        name: "hourly".into(), schedule: "0 * * * *".into(),
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::Skip,
    };
    let next = job.next_run_after(chrono::Utc::now()).unwrap();
    assert!(next > chrono::Utc::now());
}

#[test]
fn next_run_is_correct_for_every_minute() {
    let job = CronJob {
        name: "frequent".into(), schedule: "* * * * *".into(),
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::Skip,
    };
    let base = chrono::Utc::now();
    let next = job.next_run_after(base).unwrap();
    let delta = next - base;
    assert!(delta.num_seconds() >= 0 && delta.num_seconds() <= 61);
}

// ── Scheduler fire ────────────────────────────────────────────────────────────

#[tokio::test]
async fn scheduler_fires_job_at_scheduled_time() {
    use std::sync::{Arc, atomic::{AtomicU32, Ordering}};
    use std::time::Duration;

    let fired = Arc::new(AtomicU32::new(0));
    let f = Arc::clone(&fired);

    let mut scheduler = CronScheduler::new_with_callback(move |job_name| {
        f.fetch_add(1, Ordering::Relaxed);
    });

    // Use a near-future one-shot schedule (every second for testing)
    scheduler.add(CronJob {
        name: "test-job".into(), schedule: "* * * * * *".into(), // every second
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::Skip,
    }).await.unwrap();

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(fired.load(Ordering::Relaxed) >= 1);
    scheduler.shutdown().await;
}

// ── Missed run policy ─────────────────────────────────────────────────────────

#[test]
fn skip_policy_does_not_fire_for_missed_run() {
    let job = CronJob {
        name: "j".into(), schedule: "0 0 * * *".into(),
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::Skip,
    };
    let last = chrono::Utc::now() - chrono::Duration::hours(25);
    assert!(!job.should_fire_missed_run(last));
}

#[test]
fn fire_once_policy_fires_for_missed_run() {
    let job = CronJob {
        name: "j".into(), schedule: "0 0 * * *".into(),
        agent: "a".into(), goal: "g".into(),
        missed_run_policy: MissedRunPolicy::FireOnce,
    };
    let last = chrono::Utc::now() - chrono::Duration::hours(25);
    assert!(job.should_fire_missed_run(last));
}

// ── crontab.yaml parse ────────────────────────────────────────────────────────

#[test]
fn crontab_yaml_parses_correctly() {
    let yaml = r#"
apiVersion: avix/v1
kind: Crontab
jobs:
  - name: daily-report
    schedule: "0 0 * * *"
    agent: reporter
    goal: Generate daily report
    missedRunPolicy: skip
  - name: hourly-sync
    schedule: "0 * * * *"
    agent: syncer
    goal: Sync data
    missedRunPolicy: fire-once
"#;
    let crontab = avix_core::scheduler::Crontab::from_str(yaml).unwrap();
    assert_eq!(crontab.jobs.len(), 2);
    assert!(matches!(crontab.jobs[0].missed_run_policy, MissedRunPolicy::Skip));
    assert!(matches!(crontab.jobs[1].missed_run_policy, MissedRunPolicy::FireOnce));
}
```

---

## Step 3 — Implement

`CronScheduler` runs a `tokio::time::interval` loop (1-second tick), checks each job's next scheduled time against now. `CronJob.next_run_after` uses the `cron` crate. The scheduler callback fires `kernel/proc/spawn` in real usage; in tests a closure is injected.

---

## Step 4 — Verify

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-24: cron scheduler — parse, next-run, fire, missed-run policy, crontab.yaml"
```

## Success Criteria

- [ ] Valid cron expressions parse; invalid ones return error
- [ ] `next_run_after` is always in the future
- [ ] Scheduler fires callback within ~1.5s for every-second job
- [ ] `Skip` policy: no backfill for missed runs
- [ ] `FireOnce` policy: fires exactly once for missed run
- [ ] `crontab.yaml` with two jobs parses both correctly
- [ ] 12+ tests pass, 0 clippy warnings

---
---

