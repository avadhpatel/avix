/// Integration tests for in-process signal delivery.
///
/// Socket-based delivery (agent_socket / SignalDelivery) has been removed.
/// All executor signals now flow through SignalChannelRegistry.
/// See tests/signal_interruption.rs for registry-level tests.
use avix_core::{
    executor::{runtime_executor::MockToolRegistry, spawn::SpawnParams, RuntimeExecutor},
    signal::kind::SignalKind,
    types::{token::CapabilityToken, Pid},
};
use std::{sync::Arc, sync::atomic::Ordering};

// ── T-C-01: SignalKind includes SIGUSR1 and SIGUSR2 ──────────────────────────

#[test]
fn signal_kind_usr1_usr2_exist() {
    assert_eq!(SignalKind::Usr1.as_str(), "SIGUSR1");
    assert_eq!(SignalKind::Usr2.as_str(), "SIGUSR2");
}

// ── helper ──────────────────────────────────────────────────────────────────

async fn make_executor(pid: u64) -> RuntimeExecutor {
    let registry = Arc::new(MockToolRegistry::new());
    let params = SpawnParams {
        pid: Pid::from_u64(pid),
        agent_name: "signal-test".into(),
        goal: "test".into(),
        spawned_by: "alice".into(),
        session_id: "sess-1".into(),
        token: CapabilityToken::test_token(&[]),
        system_prompt: None,
        selected_model: "claude-sonnet-4".into(),
        denied_tools: vec![],
        context_limit: 0,
        runtime_dir: std::path::PathBuf::new(),
        invocation_id: String::new(),
            restore_from_pid: None,
            atp_session_id: String::new(),
    };
    RuntimeExecutor::spawn_with_registry(params, registry)
        .await
        .unwrap()
}

// ── T-C-06: deliver_signal SIGPAUSE sets paused flag ─────────────────────────

#[tokio::test]
async fn deliver_signal_pause_sets_paused_flag() {
    let executor = make_executor(42).await;
    assert!(!executor.paused.load(Ordering::Acquire));

    executor.deliver_signal("SIGPAUSE").await;

    assert!(executor.paused.load(Ordering::Acquire));
}

// ── T-C-07: deliver_signal SIGRESUME clears paused flag ──────────────────────

#[tokio::test]
async fn deliver_signal_resume_clears_paused_flag() {
    let executor = make_executor(43).await;
    executor.deliver_signal("SIGPAUSE").await;
    assert!(executor.paused.load(Ordering::Acquire));

    executor.deliver_signal("SIGRESUME").await;

    assert!(!executor.paused.load(Ordering::Acquire));
}

// ── T-C-08: deliver_signal SIGKILL sets killed flag ──────────────────────────

#[tokio::test]
async fn deliver_signal_kill_sets_killed_flag() {
    let executor = make_executor(44).await;
    assert!(!executor.killed.load(Ordering::Acquire));

    executor.deliver_signal("SIGKILL").await;

    assert!(executor.killed.load(Ordering::Acquire));
}
