/// Integration tests for the full agent spawn → turn → follow-up → stop lifecycle.
///
/// These tests wire a real ProcHandler (with InvocationStore, SessionStore, AtpEventBus)
/// to a test-specific AgentExecutorFactory that uses a scripted MockLlmClient instead of
/// connecting to llm.svc. This lets us exercise the complete lifecycle without
/// running any external services.
use std::sync::Arc;
use std::time::Duration;

use avix_core::executor::factory::AgentExecutorFactory;
use avix_core::executor::runtime_executor::{MockToolRegistry, RuntimeExecutor};
use avix_core::executor::spawn::SpawnParams;
use avix_core::gateway::atp::types::AtpEventKind;
use avix_core::gateway::event_bus::AtpEventBus;
use avix_core::invocation::{InvocationStatus, InvocationStore};
use avix_core::kernel::proc::ProcHandler;
use avix_core::llm_client::{LlmCompleteRequest, LlmCompleteResponse, LlmClient, StopReason};
use avix_core::process::entry::ProcessStatus;
use avix_core::process::table::ProcessTable;
use avix_core::session::{PersistentSessionStore, SessionStatus};
use avix_core::types::Pid;
use serde_json::json;
use tempfile::tempdir;
use tokio::time::timeout;

// ── Mock LLM ──────────────────────────────────────────────────────────────────

struct MockLlmClient {
    responses: std::sync::Mutex<Vec<LlmCompleteResponse>>,
}

impl MockLlmClient {
    fn new(responses: Vec<LlmCompleteResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _req: LlmCompleteRequest) -> anyhow::Result<LlmCompleteResponse> {
        let mut guard = self.responses.lock().unwrap();
        if guard.is_empty() {
            return Err(anyhow::anyhow!("no more mock responses"));
        }
        Ok(guard.remove(0))
    }
}

// ── Test factory ──────────────────────────────────────────────────────────────
//
// Runs a RuntimeExecutor with a MockLlmClient. After each turn it applies the
// same state transitions as IpcExecutorFactory: shutdown_with_status(Idle) on
// success or shutdown_with_status(Failed) on error.

struct TestExecutorFactory {
    responses: Arc<std::sync::Mutex<Vec<LlmCompleteResponse>>>,
    invocation_store: Arc<InvocationStore>,
    session_store: Arc<PersistentSessionStore>,
    event_bus: Arc<AtpEventBus>,
    process_table: Arc<ProcessTable>,
}

impl AgentExecutorFactory for TestExecutorFactory {
    fn launch(&self, params: SpawnParams) -> tokio::task::AbortHandle {
        let responses = Arc::clone(&self.responses);
        let istore = Arc::clone(&self.invocation_store);
        let sstore = Arc::clone(&self.session_store);
        let bus = Arc::clone(&self.event_bus);
        let table = Arc::clone(&self.process_table);

        let pid = params.pid;
        let session_id = params.session_id.clone();
        let goal = params.goal.clone();
        let inv_id = params.invocation_id.clone();

        let handle = tokio::spawn(async move {
            let registry = Arc::new(MockToolRegistry::new());
            // Take one response per invocation so multi-turn tests work correctly.
            let llm_responses: Vec<LlmCompleteResponse> = {
                let mut guard = responses.lock().unwrap();
                if guard.is_empty() { vec![] } else { vec![guard.remove(0)] }
            };
            let mock_llm = MockLlmClient::new(llm_responses);

            let mut executor =
                match RuntimeExecutor::spawn_with_registry(params, Arc::clone(&registry)).await {
                    Ok(e) => e,
                    Err(err) => {
                        let _ = table.set_status(pid, ProcessStatus::Crashed).await;
                        bus.agent_status(&session_id, pid.as_u64(), "crashed");
                        bus.agent_exit(&session_id, pid.as_u64(), 1);
                        eprintln!("executor spawn failed: {err}");
                        return;
                    }
                };

            executor = executor
                .with_event_bus(Arc::clone(&bus))
                .with_invocation_store(Arc::clone(&istore), inv_id)
                .with_session_store(Arc::clone(&sstore));

            bus.agent_status(&session_id, pid.as_u64(), "running");

            match executor.run_with_client(&goal, &mock_llm).await {
                Ok(result) => {
                    executor
                        .shutdown_with_status(
                            InvocationStatus::Idle,
                            Some("waiting_for_input".into()),
                        )
                        .await;
                    bus.agent_output(&session_id, pid.as_u64(), &result.text);
                    bus.agent_status(&session_id, pid.as_u64(), "waiting");
                    let _ = table.set_status(pid, ProcessStatus::Waiting).await;
                }
                Err(err) => {
                    executor
                        .shutdown_with_status(InvocationStatus::Failed, Some(err.to_string()))
                        .await;
                    bus.agent_status(&session_id, pid.as_u64(), "crashed");
                    bus.agent_exit(&session_id, pid.as_u64(), 1);
                    let _ = table.set_status(pid, ProcessStatus::Crashed).await;
                }
            }
        });

        handle.abort_handle()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn end_turn_response(text: &str) -> LlmCompleteResponse {
    LlmCompleteResponse {
        content: vec![json!({"type": "text", "text": text})],
        stop_reason: StopReason::EndTurn,
        input_tokens: 10,
        output_tokens: 5,
    }
}

/// Wait until the ATP event bus emits an `agent.status` event matching `status`
/// for the given pid. Panics if the event doesn't arrive within 3 seconds.
async fn wait_for_status(
    mut rx: tokio::sync::broadcast::Receiver<avix_core::gateway::event_bus::BusEvent>,
    pid: u64,
    status: &str,
) {
    let status = status.to_string();
    let pid_str = pid.to_string();
    let status_for_err = status.clone();
    timeout(Duration::from_secs(3), async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.event.event == AtpEventKind::AgentStatus => {
                    let body = &ev.event.body;
                    if body["pid"].as_str() == Some(&pid_str)
                        && body["status"].as_str() == Some(&status)
                    {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for agent.status={status_for_err} pid={pid}"));
}

async fn build_proc_handler(
    dir: &std::path::Path,
    llm_responses: Vec<LlmCompleteResponse>,
) -> (
    Arc<ProcHandler>,
    Arc<InvocationStore>,
    Arc<PersistentSessionStore>,
    Arc<AtpEventBus>,
    Arc<ProcessTable>,
) {
    let table = Arc::new(ProcessTable::new());
    let event_bus = Arc::new(AtpEventBus::new());

    let istore = Arc::new(
        InvocationStore::open(dir.join("inv.redb"))
            .await
            .unwrap(),
    );
    let sstore = Arc::new(
        PersistentSessionStore::open(dir.join("sess.redb"))
            .await
            .unwrap(),
    );

    let factory = Arc::new(TestExecutorFactory {
        responses: Arc::new(std::sync::Mutex::new(llm_responses)),
        invocation_store: Arc::clone(&istore),
        session_store: Arc::clone(&sstore),
        event_bus: Arc::clone(&event_bus),
        process_table: Arc::clone(&table),
    });

    let master_key = b"test-master-key-32-bytes-padded!".to_vec();
    let handler = Arc::new(
        ProcHandler::new_with_factory(
            Arc::clone(&table),
            dir.join("agents.yaml"),
            master_key,
            dir.join("run"),
            factory,
        )
        .with_invocation_store(Arc::clone(&istore))
        .with_session_store(Arc::clone(&sstore)),
    );

    (handler, istore, sstore, event_bus, table)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Spawn an agent → invocation and session created → executor completes first turn →
/// invocation goes Idle, session goes Idle, process goes Waiting, ATP "waiting" event emitted.
#[tokio::test]
async fn spawn_creates_records_and_first_turn_transitions_to_idle() {
    let dir = tempdir().unwrap();
    let (handler, istore, sstore, bus, table) = build_proc_handler(
        dir.path(),
        vec![end_turn_response("Paris is the capital of France.")],
    )
    .await;

    let rx = bus.subscribe();

    let pid = handler
        .spawn("researcher", "What is the capital of France?", "", "alice", None)
        .await
        .unwrap();

    // Invocation created with Running status immediately after spawn.
    let inv_id = {
        let invocations = istore.list_for_user("alice").await.unwrap();
        assert_eq!(invocations.len(), 1, "one invocation must be created");
        let inv = &invocations[0];
        assert_eq!(inv.agent_name, "researcher");
        assert_eq!(inv.status, InvocationStatus::Running);
        inv.id.clone()
    };

    // Session created with Running status.
    let session_id = {
        let sessions = sstore.list_for_user("alice").await.unwrap();
        assert_eq!(sessions.len(), 1, "one session must be created");
        let sess = &sessions[0];
        assert_eq!(sess.status, SessionStatus::Running);
        assert_eq!(sess.owner_pid, pid, "spawning pid must be session owner");
        assert!(sess.pids.contains(&pid));
        sess.id
    };

    // Wait for the executor to finish and emit "waiting".
    wait_for_status(rx, pid, "waiting").await;

    // Invocation → Idle (non-terminal, no ended_at).
    let inv = istore.get(&inv_id).await.unwrap().unwrap();
    assert_eq!(inv.status, InvocationStatus::Idle);
    assert!(inv.ended_at.is_none(), "Idle must not set ended_at");

    // Session → Idle.
    let sess = sstore.get(&session_id).await.unwrap().unwrap();
    assert_eq!(sess.status, SessionStatus::Idle);

    // Process → Waiting.
    let entry = table.get(Pid::from_u64(pid)).await.unwrap();
    assert_eq!(entry.status, ProcessStatus::Waiting);
}

/// After first turn completes (Idle), resume_session spawns a new invocation in the
/// same session. The second invocation processes the follow-up and also reaches Idle.
/// Both invocations share the same session_id.
#[tokio::test]
async fn follow_up_via_resume_session_reuses_session() {
    let dir = tempdir().unwrap();
    let (handler, istore, sstore, bus, _table) = build_proc_handler(
        dir.path(),
        vec![
            end_turn_response("Paris is the capital of France."),
            end_turn_response("Paris was founded around 250 BC by the Parisii tribe."),
        ],
    )
    .await;

    // ── Turn 1 ────────────────────────────────────────────────────────────────
    let rx1 = bus.subscribe();
    let pid1 = handler
        .spawn("researcher", "What is the capital of France?", "", "alice", None)
        .await
        .unwrap();

    wait_for_status(rx1, pid1, "waiting").await;

    let session_id = sstore.list_for_user("alice").await.unwrap()[0].id;

    // ── Turn 2: follow-up in same session via resume ───────────────────────────
    let rx2 = bus.subscribe();
    let pid2 = handler
        .resume_session(&session_id, Some("Tell me more about its history."))
        .await
        .unwrap();

    wait_for_status(rx2, pid2, "waiting").await;

    // Both invocations belong to the same session.
    let invocations = istore.list_for_user("alice").await.unwrap();
    assert_eq!(invocations.len(), 2, "two invocations for the session");
    assert!(
        invocations.iter().all(|r| r.session_id == session_id.to_string()),
        "both invocations must share the session_id"
    );
    assert!(
        invocations.iter().all(|r| r.status == InvocationStatus::Idle),
        "both invocations must be Idle after their turns"
    );

    // Session is still alive (Idle, not terminal).
    let sess = sstore.get(&session_id).await.unwrap().unwrap();
    assert_eq!(sess.status, SessionStatus::Idle);
}

/// ATP events are emitted in the correct sequence for a successful turn:
/// running → (output) → waiting.
#[tokio::test]
async fn atp_events_emitted_in_correct_order() {
    let dir = tempdir().unwrap();
    let (handler, _istore, _sstore, bus, _table) = build_proc_handler(
        dir.path(),
        vec![end_turn_response("42 is the answer.")],
    )
    .await;

    let mut rx = bus.subscribe();
    let pid = handler
        .spawn("solver", "What is the answer?", "", "alice", None)
        .await
        .unwrap();

    let mut statuses: Vec<String> = Vec::new();
    let mut output_text: Option<String> = None;
    let pid_str = pid.to_string();

    timeout(Duration::from_secs(3), async {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.event.event == AtpEventKind::AgentStatus => {
                    let body = &ev.event.body;
                    if body["pid"].as_str() == Some(&pid_str) {
                        let s = body["status"].as_str().unwrap_or("").to_string();
                        statuses.push(s.clone());
                        if s == "waiting" {
                            break;
                        }
                    }
                }
                Ok(ev) if ev.event.event == AtpEventKind::AgentOutput => {
                    let body = &ev.event.body;
                    if body["pid"].as_str() == Some(&pid_str) {
                        output_text = body["text"].as_str().map(|s| s.to_string());
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    })
    .await
    .expect("timed out waiting for events");

    assert!(statuses.contains(&"running".to_string()), "must emit running");
    assert!(statuses.contains(&"waiting".to_string()), "must emit waiting");

    let run_idx = statuses.iter().position(|s| s == "running").unwrap();
    let wait_idx = statuses.iter().position(|s| s == "waiting").unwrap();
    assert!(run_idx < wait_idx, "running must precede waiting");

    let text = output_text.expect("agent_output event must be emitted");
    assert!(text.contains("42"), "output must contain LLM response text");
}

/// Aborting the owner PID finalizes the invocation as Killed and the session as Failed.
#[tokio::test]
async fn abort_agent_kills_invocation_and_fails_session() {
    let dir = tempdir().unwrap();
    let (handler, istore, sstore, bus, table) = build_proc_handler(
        dir.path(),
        vec![end_turn_response("first turn")],
    )
    .await;

    // Spawn and wait for the first turn to complete (Waiting).
    let rx = bus.subscribe();
    let pid = handler
        .spawn("researcher", "Some task", "", "alice", None)
        .await
        .unwrap();
    wait_for_status(rx, pid, "waiting").await;

    let session_id = sstore.list_for_user("alice").await.unwrap()[0].id;

    // Abort the agent (owner PID).
    handler.abort_agent(pid).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Invocation → Killed.
    let invocations = istore.list_for_user("alice").await.unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(
        invocations[0].status,
        InvocationStatus::Killed,
        "aborted invocation must be Killed"
    );

    // Session → Failed (owner PID was killed).
    let sess = sstore.get(&session_id).await.unwrap().unwrap();
    assert_eq!(
        sess.status,
        SessionStatus::Failed,
        "session must be Failed after owner PID is killed"
    );

    // Process → Stopped.
    let entry = table.get(Pid::from_u64(pid)).await.unwrap();
    assert_eq!(entry.status, ProcessStatus::Stopped);
}

/// Full lifecycle: spawn → turn 1 idle → follow-up turn 2 idle → stop via abort.
#[tokio::test]
async fn full_lifecycle_spawn_followup_stop() {
    let dir = tempdir().unwrap();
    let (handler, istore, sstore, bus, table) = build_proc_handler(
        dir.path(),
        vec![
            end_turn_response("The capital of France is Paris."),
            end_turn_response("Paris is known for the Eiffel Tower."),
        ],
    )
    .await;

    // ── Turn 1 ────────────────────────────────────────────────────────────────
    let rx1 = bus.subscribe();
    let pid1 = handler
        .spawn("researcher", "Capital of France?", "", "alice", None)
        .await
        .unwrap();
    wait_for_status(rx1, pid1, "waiting").await;

    let session_id = sstore.list_for_user("alice").await.unwrap()[0].id;

    let invocations = istore.list_for_user("alice").await.unwrap();
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].status, InvocationStatus::Idle);

    // ── Turn 2: follow-up ─────────────────────────────────────────────────────
    let rx2 = bus.subscribe();
    let pid2 = handler
        .resume_session(&session_id, Some("What is it famous for?"))
        .await
        .unwrap();
    wait_for_status(rx2, pid2, "waiting").await;

    let invocations = istore.list_for_user("alice").await.unwrap();
    assert_eq!(invocations.len(), 2);
    assert!(invocations.iter().all(|r| r.status == InvocationStatus::Idle));

    // ── Stop: abort the session owner (pid1) to close the session ─────────────
    handler.abort_agent(pid1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Owner invocation → Killed.
    let inv1 = istore
        .list_for_user("alice")
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.pid == pid1)
        .expect("first invocation must exist");
    assert_eq!(inv1.status, InvocationStatus::Killed);

    // Session → Failed because owner PID was killed.
    let sess = sstore.get(&session_id).await.unwrap().unwrap();
    assert_eq!(sess.status, SessionStatus::Failed);

    // Process table reflects stopped for pid1.
    let entry1 = table.get(Pid::from_u64(pid1)).await.unwrap();
    assert_eq!(entry1.status, ProcessStatus::Stopped);
}
