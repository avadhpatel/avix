use std::sync::Arc;
use tracing::{info, instrument};

use crate::error::AvixError;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::session::record::SessionStatus;
use crate::session::PersistentSessionStore;

/// Phase 2.5 — repair session records left stale by the previous run.
///
/// Invocations are NOT modified here; they will be restored as live executors in
/// phase 3.5 (`ProcHandler::restore_interrupted_agents`) after the real ToolRegistry
/// is available.
///
/// Algorithm:
///   1. Scan all invocations to find session IDs that had Running or Paused agents.
///   2. For each affected session: clear `pids` (all executors are gone after restart)
///      and transition the session status:
///      `Running` → `Idle`  (live agents will be re-attached in phase 3.5)
///      `Paused`  → `Idle`  (in-memory pause flag is lost; agent resumes normally)
#[instrument(skip_all)]
pub async fn phase3_crash_recovery(
    invocation_store: Arc<InvocationStore>,
    session_store: Arc<PersistentSessionStore>,
) -> Result<(), AvixError> {
    info!("phase 2.5: repairing stale session records from previous run");

    let invocations = invocation_store.list_all().await?;
    let mut affected_sessions: std::collections::HashSet<String> = Default::default();

    for inv in &invocations {
        if matches!(
            inv.status,
            InvocationStatus::Running | InvocationStatus::Paused | InvocationStatus::Idle
        ) {
            affected_sessions.insert(inv.session_id.clone());
        }
    }

    // Repair affected sessions: clear stale PIDs and transition to Idle.
    let mut sessions_repaired = 0u32;
    for session_id_str in &affected_sessions {
        let session_uuid = match uuid::Uuid::parse_str(session_id_str) {
            Ok(u) => u,
            Err(_) => continue,
        };
        if let Ok(Some(mut session)) = session_store.get(&session_uuid).await {
            // All executor tasks are dead after restart — clear stale PIDs so
            // restore_interrupted_agents can add fresh ones without collisions.
            session.pids.clear();

            match session.status {
                SessionStatus::Running | SessionStatus::Paused => {
                    session.mark_idle();
                    sessions_repaired += 1;
                }
                _ => {}
            }
            let _ = session_store.update(&session).await;
        }
    }

    info!(
        sessions_repaired,
        "phase 2.5: session repair complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ── phase3_crash_recovery tests ───────────────────────────────────────────

    async fn open_inv_store(dir: &TempDir) -> Arc<InvocationStore> {
        Arc::new(
            InvocationStore::open(dir.path().join("invocations.redb"))
                .await
                .unwrap(),
        )
    }

    async fn open_sess_store(dir: &TempDir) -> Arc<PersistentSessionStore> {
        Arc::new(
            PersistentSessionStore::open(dir.path().join("sessions.redb"))
                .await
                .unwrap(),
        )
    }

    fn make_inv(id: &str, session_id: &str, status: InvocationStatus) -> crate::invocation::InvocationRecord {
        let mut r = crate::invocation::InvocationRecord::new(
            id.into(),
            "agent".into(),
            "alice".into(),
            1,
            "goal".into(),
            session_id.into(),
        );
        r.status = status;
        r
    }

    fn make_session(id: uuid::Uuid, status: SessionStatus) -> crate::session::record::SessionRecord {
        let mut s = crate::session::record::SessionRecord::new(
            id,
            "alice".into(),
            "agent".into(),
            "title".into(),
            "goal".into(),
            42,
        );
        s.status = status;
        s
    }

    // ── Invocations are NOT modified — they are restored as live executors in phase 3.5.

    #[tokio::test]
    async fn running_invocations_are_preserved_for_restore() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("inv-1", "sess-1", InvocationStatus::Running)).await.unwrap();
        inv.create(&make_inv("inv-2", "sess-2", InvocationStatus::Running)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        // Invocation statuses must be unchanged — restore happens in phase 3.5.
        assert_eq!(inv.get("inv-1").await.unwrap().unwrap().status, InvocationStatus::Running);
        assert_eq!(inv.get("inv-2").await.unwrap().unwrap().status, InvocationStatus::Running);
    }

    #[tokio::test]
    async fn paused_invocations_are_preserved_for_restore() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("inv-p", "sess-p", InvocationStatus::Paused)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        assert_eq!(inv.get("inv-p").await.unwrap().unwrap().status, InvocationStatus::Paused);
    }

    #[tokio::test]
    async fn terminal_invocations_are_untouched() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("done", "s", InvocationStatus::Completed)).await.unwrap();
        inv.create(&make_inv("fail", "s", InvocationStatus::Failed)).await.unwrap();
        inv.create(&make_inv("kill", "s", InvocationStatus::Killed)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        assert_eq!(inv.get("done").await.unwrap().unwrap().status, InvocationStatus::Completed);
        assert_eq!(inv.get("fail").await.unwrap().unwrap().status, InvocationStatus::Failed);
        assert_eq!(inv.get("kill").await.unwrap().unwrap().status, InvocationStatus::Killed);
    }

    #[tokio::test]
    async fn running_session_becomes_idle_and_pids_cleared() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        let mut session = make_session(sid, SessionStatus::Running);
        session.pids = vec![42];
        sess.create(&session).await.unwrap();

        inv.create(&make_inv("inv-r", &sid.to_string(), InvocationStatus::Running)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        let updated = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(updated.status, SessionStatus::Idle);
        assert!(updated.pids.is_empty());
    }

    #[tokio::test]
    async fn paused_session_becomes_idle() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        sess.create(&make_session(sid, SessionStatus::Paused)).await.unwrap();

        inv.create(&make_inv("inv-pa", &sid.to_string(), InvocationStatus::Paused)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        let updated = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(updated.status, SessionStatus::Idle);
    }

    #[tokio::test]
    async fn idle_session_with_idle_invocation_pids_cleared() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        let mut session = make_session(sid, SessionStatus::Idle);
        session.pids = vec![99];
        sess.create(&session).await.unwrap();
        inv.create(&make_inv("inv-i", &sid.to_string(), InvocationStatus::Idle)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        // Idle sessions with non-terminal invocations have stale pids cleared.
        let s = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Idle);
        assert!(s.pids.is_empty());
    }

    #[tokio::test]
    async fn session_with_only_terminal_invocations_is_untouched() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        sess.create(&make_session(sid, SessionStatus::Idle)).await.unwrap();
        inv.create(&make_inv("done", &sid.to_string(), InvocationStatus::Completed)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        // Session not in affected_sessions (no non-terminal invocation) — untouched.
        let s = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[tokio::test]
    async fn crash_recovery_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        let mut session = make_session(sid, SessionStatus::Running);
        session.pids = vec![42];
        sess.create(&session).await.unwrap();
        inv.create(&make_inv("inv-x", &sid.to_string(), InvocationStatus::Running)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();
        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        // After two passes the session is still Idle and pids still empty.
        let s = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Idle);
        assert!(s.pids.is_empty());
        // Invocation is still unchanged (to be restored in phase 3.5).
        assert_eq!(inv.get("inv-x").await.unwrap().unwrap().status, InvocationStatus::Running);
    }
}
