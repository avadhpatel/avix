/// Integration tests for the jobs service (Gap D).
use avix_core::{
    jobs::{handle_job_watch, start_job, JobError, JobRegistry, JobState},
    types::Pid,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

fn make_registry() -> Arc<RwLock<JobRegistry>> {
    Arc::new(RwLock::new(JobRegistry::new().0))
}

// ── T-D-01: Create job starts in Pending state ────────────────────────────────

#[tokio::test]
async fn create_job_is_pending() {
    let reg = make_registry();
    let id = reg.write().await.create("fs/write", Pid::from_u64(10));
    assert_eq!(reg.read().await.get(&id).unwrap().state, JobState::Pending);
}

// ── T-D-02: start() transitions Pending → Running and emits StatusChange ──────

#[tokio::test]
async fn start_job_transitions_to_running() {
    let (mut reg, mut rx) = JobRegistry::new();
    let id = reg.create("fs/write", Pid::from_u64(10));
    reg.start(&id).unwrap();

    assert_eq!(reg.get(&id).unwrap().state, JobState::Running);

    // Should have received a StatusChange event.
    let event = rx.0.try_recv().unwrap();
    assert!(
        matches!(
            event,
            avix_core::jobs::JobEvent::StatusChange {
                new_state: JobState::Running,
                ..
            }
        ),
        "unexpected event: {event:?}"
    );
}

// ── T-D-03: progress() emits Progress event without state change ──────────────

#[tokio::test]
async fn progress_event_emitted() {
    let (mut reg, mut rx) = JobRegistry::new();
    let id = reg.create("fs/write", Pid::from_u64(10));
    reg.start(&id).unwrap();
    let _ = rx.0.try_recv(); // consume StatusChange

    reg.progress(&id, Some(50), Some("encoding".into()), None)
        .unwrap();

    assert_eq!(reg.get(&id).unwrap().state, JobState::Running);
    let event = rx.0.try_recv().unwrap();
    assert!(
        matches!(
            event,
            avix_core::jobs::JobEvent::Progress {
                percent: Some(50),
                ..
            }
        ),
        "unexpected event: {event:?}"
    );
}

// ── T-D-04: complete() transitions Running → Done ────────────────────────────

#[tokio::test]
async fn complete_job_transitions_to_done() {
    let (mut reg, mut rx) = JobRegistry::new();
    let id = reg.create("tool", Pid::from_u64(10));
    reg.start(&id).unwrap();
    let _ = rx.0.try_recv();

    reg.complete(&id, json!({"bytes": 42})).unwrap();

    assert_eq!(reg.get(&id).unwrap().state, JobState::Done);

    // StatusChange then Complete events.
    let e1 = rx.0.try_recv().unwrap();
    assert!(matches!(
        e1,
        avix_core::jobs::JobEvent::StatusChange {
            new_state: JobState::Done,
            ..
        }
    ));
    let e2 = rx.0.try_recv().unwrap();
    assert!(matches!(e2, avix_core::jobs::JobEvent::Complete { .. }));
}

// ── T-D-05: fail() transitions Running → Failed ──────────────────────────────

#[tokio::test]
async fn fail_job_transitions_to_failed() {
    let (mut reg, mut rx) = JobRegistry::new();
    let id = reg.create("tool", Pid::from_u64(10));
    reg.start(&id).unwrap();
    let _ = rx.0.try_recv();

    reg.fail(
        &id,
        JobError {
            code: -32001,
            message: "codec missing".into(),
        },
    )
    .unwrap();

    assert_eq!(reg.get(&id).unwrap().state, JobState::Failed);
    let e1 = rx.0.try_recv().unwrap();
    assert!(matches!(
        e1,
        avix_core::jobs::JobEvent::StatusChange {
            new_state: JobState::Failed,
            ..
        }
    ));
    let e2 = rx.0.try_recv().unwrap();
    assert!(matches!(e2, avix_core::jobs::JobEvent::Fail { .. }));
}

// ── T-D-06: Cannot complete a Done job ───────────────────────────────────────

#[tokio::test]
async fn complete_done_job_returns_error() {
    let reg = make_registry();
    let id = {
        let mut r = reg.write().await;
        let id = r.create("tool", Pid::from_u64(10));
        r.start(&id).unwrap();
        r.complete(&id, json!({})).unwrap();
        id
    };

    let result = reg.write().await.complete(&id, json!({}));
    assert!(result.is_err(), "should fail on double-complete");
}

// ── T-D-07: cancel() transitions Running → Cancelled ─────────────────────────

#[tokio::test]
async fn cancel_running_job() {
    let reg = make_registry();
    let id = {
        let mut r = reg.write().await;
        let id = r.create("tool", Pid::from_u64(10));
        r.start(&id).unwrap();
        id
    };

    reg.write().await.cancel(&id).unwrap();
    assert_eq!(
        reg.read().await.get(&id).unwrap().state,
        JobState::Cancelled
    );
}

// ── T-D-08: job/watch returns next event within timeout ──────────────────────

#[tokio::test]
async fn job_watch_returns_next_event() {
    let reg = make_registry();
    let id = {
        let mut r = reg.write().await;
        let id = r.create("fs/write", Pid::from_u64(10));
        r.start(&id).unwrap();
        id
    };

    // Emit a progress event after a short delay.
    let reg_clone = reg.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        reg_clone
            .write()
            .await
            .progress(&id_clone, Some(33), Some("stage1".into()), None)
            .unwrap();
    });

    let result = handle_job_watch(id, Some(500), reg).await.unwrap();
    assert!(
        result.get("event").is_some(),
        "expected event, got: {result}"
    );
    assert_eq!(result["event"]["type"], "progress");
}

// ── T-D-09: job/watch times out if no event ──────────────────────────────────

#[tokio::test]
async fn job_watch_times_out() {
    let reg = make_registry();
    let id = {
        let mut r = reg.write().await;
        let id = r.create("fs/write", Pid::from_u64(10));
        r.start(&id).unwrap();
        id
    };

    let result = handle_job_watch(id, Some(50), reg).await.unwrap();
    assert_eq!(result["status"], "timeout");
}

// ── T-D-10: job/watch returns not_found for unknown job ──────────────────────

#[tokio::test]
async fn job_watch_unknown_job() {
    let reg = make_registry();
    let result = handle_job_watch("ghost-job-id".into(), Some(50), reg)
        .await
        .unwrap();
    assert_eq!(result["status"], "not_found");
}

// ── T-D-11: start_job helper runs background work to completion ───────────────

#[tokio::test]
async fn start_job_helper_runs_to_completion() {
    let reg = make_registry();
    let mut rx = reg.read().await.subscribe();

    let id = start_job(
        "fs/write",
        Pid::from_u64(10),
        reg.clone(),
        |job_id, registry| async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            registry
                .write()
                .await
                .complete(&job_id, json!({"bytes": 42}))
                .unwrap();
        },
    )
    .await;

    // Wait for Complete event.
    let found = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            match rx.recv().await {
                Ok(avix_core::jobs::JobEvent::Complete { job_id, .. }) if job_id == id => {
                    break true
                }
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .expect("timeout waiting for Complete event");

    assert!(found);
    assert_eq!(reg.read().await.get(&id).unwrap().state, JobState::Done);
}

// ── T-D-12: Concurrent jobs are tracked independently ────────────────────────

#[tokio::test]
async fn concurrent_jobs_are_independent() {
    let reg = make_registry();

    let mut ids = Vec::new();
    for i in 0..5u64 {
        let id = start_job(
            "concurrent/op",
            Pid::from_u64(i),
            reg.clone(),
            |job_id, registry| async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                registry
                    .write()
                    .await
                    .complete(&job_id, json!({"worker": job_id}))
                    .unwrap();
            },
        )
        .await;
        ids.push(id);
    }

    // Wait for all jobs to complete.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    for id in &ids {
        assert_eq!(
            reg.read().await.get(id).unwrap().state,
            JobState::Done,
            "job {id} not done"
        );
    }
}
