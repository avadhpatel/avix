use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::error::AvixError;
use crate::invocation::{InvocationStatus, InvocationStore};
use crate::kernel::proc::ProcHandler;
use crate::process::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::process::table::ProcessTable;
use crate::session::record::SessionStatus;
use crate::session::PersistentSessionStore;
use crate::types::Pid;

/// Kernel boot phase 3: re-adopt orphaned agents from agents.yaml.
/// Any PID in agents.yaml that is not yet registered in the process table is re-adopted
/// as Running. PIDs already present in the table are skipped (idempotent).
/// Links: docs/dev_plans/PROJECT-SPAWN-001-dev-plan.md#detailed-implementation-guidance
pub async fn phase3_re_adopt(
    process_table: Arc<ProcessTable>,
    agents_yaml_path: PathBuf,
    master_key: Vec<u8>,
) -> Result<(), AvixError> {
    info!("phase 3: re-adopting orphaned agents");

    let handler = ProcHandler::new(process_table.clone(), agents_yaml_path, master_key);

    // Load persisted agents
    let agents_yaml = match handler.load_agents_yaml().await {
        Ok(yaml) => yaml,
        Err(e) => {
            warn!("failed to load agents.yaml: {}", e);
            return Ok(()); // Not a fatal error
        }
    };

    let mut re_adopted = 0;
    for record in &agents_yaml.agents {
        // Liveness check: consult the kernel process table, not the host OS.
        // Avix PIDs are virtual — they have no meaning as host OS process IDs.
        if process_table.get(Pid::new(record.pid)).await.is_some() {
            info!(pid = record.pid, name = %record.name, "agent already registered, skipping re-adopt");
            continue;
        }

        info!(pid = record.pid, name = %record.name, "re-adopting agent from agents.yaml");

        let entry = ProcessEntry {
            pid: Pid::new(record.pid),
            name: record.name.clone(),
            kind: ProcessKind::Agent,
            status: ProcessStatus::Running,
            parent: None,
            spawned_by_user: "kernel".to_string(),
            goal: record.goal.clone(),
            spawned_at: record.spawned_at,
            ..Default::default()
        };

        process_table.insert(entry).await;

        // TODO: Rewrite /proc/<pid>/ files
        // TODO: Send SIGSTART to resume IPC

        re_adopted += 1;
    }

    info!(re_adopted, "re-adopted agents from agents.yaml");
    Ok(())
}

/// Phase 2.5 — fix stale invocation and session records from the previous run.
///
/// Must run before any agents are spawned and before the ATP gateway starts,
/// so no client ever observes a Running/Paused record that has no live executor.
///
/// Algorithm:
///   1. Scan all invocations; for each `Running` or `Paused`: finalize as `Killed`.
///   2. Collect the session IDs that were affected.
///   3. For each affected session: clear `pids`, then transition status:
///      `Running`  → `Idle`  (allow user to resume via `session resume`)
///      `Paused`   → `Idle`  (in-memory pause state is lost; allow resumption)
pub async fn phase3_crash_recovery(
    invocation_store: Arc<InvocationStore>,
    session_store: Arc<PersistentSessionStore>,
) -> Result<(), AvixError> {
    info!("phase 2.5: scanning for stale records from previous run");

    let invocations = invocation_store.list_all().await?;
    let mut killed = 0u32;
    let mut affected_sessions: std::collections::HashSet<String> = Default::default();

    for inv in &invocations {
        if matches!(
            inv.status,
            InvocationStatus::Running | InvocationStatus::Paused
        ) {
            info!(
                id = %inv.id,
                agent = %inv.agent_name,
                status = ?inv.status,
                "marking stale invocation as killed"
            );
            let _ = invocation_store
                .finalize(
                    &inv.id,
                    InvocationStatus::Killed,
                    chrono::Utc::now(),
                    inv.tokens_consumed,
                    inv.tool_calls_total,
                    Some("interrupted_at_shutdown".into()),
                )
                .await;
            killed += 1;
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
            // All executor tasks are dead after restart — clear the PID list.
            session.pids.clear();

            // Transition non-terminal states to Idle so the user can resume.
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
        killed,
        sessions_repaired,
        "phase 2.5: crash recovery complete"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_agents_yaml(
        agents: Vec<crate::kernel::proc::AgentRecord>,
    ) -> crate::kernel::proc::AgentsYaml {
        crate::kernel::proc::AgentsYaml { agents }
    }

    fn make_record(pid: u32, name: &str) -> crate::kernel::proc::AgentRecord {
        crate::kernel::proc::AgentRecord {
            pid,
            name: name.to_string(),
            goal: "test goal".to_string(),
            session_id: "sess-1".to_string(),
            spawned_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn re_adopt_registers_agents_not_in_process_table() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());

        let yaml_str = serde_yaml::to_string(&make_agents_yaml(vec![
            make_record(10, "agent-a"),
            make_record(11, "agent-b"),
        ]))
        .unwrap();
        std::fs::write(&yaml_path, yaml_str).unwrap();

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        let entries = table.list_by_kind(ProcessKind::Agent).await;
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e.pid.as_u32() == 10 && e.name == "agent-a"));
        assert!(entries
            .iter()
            .any(|e| e.pid.as_u32() == 11 && e.name == "agent-b"));
        assert!(entries.iter().all(|e| e.status == ProcessStatus::Running));
    }

    #[tokio::test]
    async fn re_adopt_skips_pids_already_in_process_table() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml");
        let table = Arc::new(ProcessTable::new());

        // Pre-register PID 20 so it's already in the table
        table
            .insert(ProcessEntry {
                pid: Pid::new(20),
                name: "already-running".to_string(),
                kind: ProcessKind::Agent,
                status: ProcessStatus::Running,
                goal: "original goal".to_string(),
                spawned_by_user: "kernel".to_string(),
                spawned_at: chrono::Utc::now(),
                ..Default::default()
            })
            .await;

        let yaml_str = serde_yaml::to_string(&make_agents_yaml(vec![
            make_record(20, "already-running"),
            make_record(21, "new-agent"),
        ]))
        .unwrap();
        std::fs::write(&yaml_path, yaml_str).unwrap();

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        // PID 20 was already there (not duplicated), PID 21 was added
        let entries = table.list_by_kind(ProcessKind::Agent).await;
        assert_eq!(entries.len(), 2);
        // PID 20 keeps its original entry (not replaced)
        let e20 = entries.iter().find(|e| e.pid.as_u32() == 20).unwrap();
        assert_eq!(e20.name, "already-running");
        assert!(entries.iter().any(|e| e.pid.as_u32() == 21));
    }

    #[tokio::test]
    async fn re_adopt_is_noop_when_agents_yaml_missing() {
        let dir = TempDir::new().unwrap();
        let yaml_path = dir.path().join("agents.yaml"); // does not exist
        let table = Arc::new(ProcessTable::new());

        phase3_re_adopt(table.clone(), yaml_path, vec![0u8; 32])
            .await
            .unwrap();

        assert!(table.list_by_kind(ProcessKind::Agent).await.is_empty());
    }

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

    #[tokio::test]
    async fn running_invocations_become_killed() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("inv-1", "sess-1", InvocationStatus::Running)).await.unwrap();
        inv.create(&make_inv("inv-2", "sess-2", InvocationStatus::Running)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        let r1 = inv.get("inv-1").await.unwrap().unwrap();
        let r2 = inv.get("inv-2").await.unwrap().unwrap();
        assert_eq!(r1.status, InvocationStatus::Killed);
        assert_eq!(r1.exit_reason.as_deref(), Some("interrupted_at_shutdown"));
        assert_eq!(r2.status, InvocationStatus::Killed);
    }

    #[tokio::test]
    async fn paused_invocations_become_killed() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("inv-p", "sess-p", InvocationStatus::Paused)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        let r = inv.get("inv-p").await.unwrap().unwrap();
        assert_eq!(r.status, InvocationStatus::Killed);
        assert_eq!(r.exit_reason.as_deref(), Some("interrupted_at_shutdown"));
    }

    #[tokio::test]
    async fn idle_and_terminal_invocations_are_untouched() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("idle", "s", InvocationStatus::Idle)).await.unwrap();
        inv.create(&make_inv("done", "s", InvocationStatus::Completed)).await.unwrap();
        inv.create(&make_inv("fail", "s", InvocationStatus::Failed)).await.unwrap();
        inv.create(&make_inv("kill", "s", InvocationStatus::Killed)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        assert_eq!(inv.get("idle").await.unwrap().unwrap().status, InvocationStatus::Idle);
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
    async fn session_with_only_idle_invocations_is_untouched() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        let sid = uuid::Uuid::new_v4();
        sess.create(&make_session(sid, SessionStatus::Idle)).await.unwrap();
        inv.create(&make_inv("inv-i", &sid.to_string(), InvocationStatus::Idle)).await.unwrap();

        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        // Session was not in affected_sessions so it is not touched.
        let s = sess.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[tokio::test]
    async fn crash_recovery_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let inv = open_inv_store(&dir).await;
        let sess = open_sess_store(&dir).await;

        inv.create(&make_inv("inv-x", "sess-x", InvocationStatus::Running)).await.unwrap();

        // First pass
        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();
        // Second pass — all records already Killed, should be a no-op
        phase3_crash_recovery(Arc::clone(&inv), Arc::clone(&sess)).await.unwrap();

        let r = inv.get("inv-x").await.unwrap().unwrap();
        assert_eq!(r.status, InvocationStatus::Killed);
    }
}
