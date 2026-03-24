use avix_core::executor::hil::{CapabilityUpgrader, Escalator, HilApprover};
use avix_core::kernel::ApprovalTokenStore;
use avix_core::signal::kind::SignalKind;
use avix_core::signal::{Signal, SignalBus};
use avix_core::types::{token::CapabilityToken, Pid};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn make_signal(pid_val: u32, hil_id: &str, decision: &str) -> Signal {
    Signal {
        target: Pid::new(pid_val),
        kind: SignalKind::Resume,
        payload: json!({
            "hilId": hil_id,
            "decision": decision
        }),
    }
}

// ---- HIL Approval tests ----

#[tokio::test]
async fn hil_approval_approved() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(1), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(make_signal(1, "hil-1", "approved"))
            .await
            .unwrap();
    });

    let result = approver
        .await_approval("hil-1", Duration::from_secs(2))
        .await
        .unwrap();
    assert!(result.approved);
}

#[tokio::test]
async fn hil_approval_denied() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(2), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(make_signal(2, "hil-2", "denied")).await.unwrap();
    });

    let result = approver
        .await_approval("hil-2", Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!result.approved);
}

#[tokio::test]
async fn hil_approval_timeout() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(3), Arc::clone(&bus));
    // No signal sent — should timeout
    let result = approver
        .await_approval("hil-timeout", Duration::from_millis(50))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("timed out"));
}

#[tokio::test]
async fn hil_approval_wrong_hil_id_ignored() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(4), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Send wrong hil_id first
        bus2.send(make_signal(4, "wrong-id", "approved"))
            .await
            .unwrap();
        // Then correct one
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(make_signal(4, "hil-4", "approved"))
            .await
            .unwrap();
    });

    let result = approver
        .await_approval("hil-4", Duration::from_secs(2))
        .await
        .unwrap();
    assert!(result.approved);
}

#[tokio::test]
async fn hil_approval_note_and_reason_passed_through() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(5), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(Signal {
            target: Pid::new(5),
            kind: SignalKind::Resume,
            payload: json!({
                "hilId": "hil-5",
                "decision": "denied",
                "reason": "too risky",
                "note": "please review"
            }),
        })
        .await
        .unwrap();
    });

    let result = approver
        .await_approval("hil-5", Duration::from_secs(2))
        .await
        .unwrap();
    assert!(!result.approved);
    assert_eq!(result.denial_reason.as_deref(), Some("too risky"));
    assert_eq!(result.note.as_deref(), Some("please review"));
}

// ---- Capability upgrade tests ----

#[tokio::test]
async fn cap_upgrade_approved_updates_token() {
    let bus = Arc::new(SignalBus::new());
    let initial_token = CapabilityToken::test_token(&["fs/read"]);
    let mut upgrader = CapabilityUpgrader::new(Pid::new(10), initial_token, Arc::clone(&bus));

    let new_token = CapabilityToken::test_token(&["fs/read", "fs/write"]);

    let bus2 = Arc::clone(&bus);
    let new_token_val = serde_json::to_value(&new_token).unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(Signal {
            target: Pid::new(10),
            kind: SignalKind::Resume,
            payload: json!({
                "hilId": "cap-1",
                "decision": "approved",
                "new_capability_token": new_token_val
            }),
        })
        .await
        .unwrap();
    });

    upgrader
        .request_tool(
            "fs/write",
            "need write access",
            "cap-1",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert!(upgrader.current_token().has_tool("fs/write"));
}

#[tokio::test]
async fn cap_upgrade_denied_returns_error() {
    let bus = Arc::new(SignalBus::new());
    let token = CapabilityToken::test_token(&[]);
    let mut upgrader = CapabilityUpgrader::new(Pid::new(11), token, Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(make_signal(11, "cap-denied", "denied"))
            .await
            .unwrap();
    });

    let result = upgrader
        .request_tool(
            "fs/write",
            "need write",
            "cap-denied",
            Duration::from_secs(2),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("denied"));
}

#[tokio::test]
async fn cap_upgrade_timeout_returns_error() {
    let bus = Arc::new(SignalBus::new());
    let token = CapabilityToken::test_token(&[]);
    let mut upgrader = CapabilityUpgrader::new(Pid::new(12), token, Arc::clone(&bus));
    let result = upgrader
        .request_tool(
            "fs/write",
            "need write",
            "cap-timeout",
            Duration::from_millis(50),
        )
        .await;
    assert!(result.is_err());
}

// ---- Escalation tests ----

#[tokio::test]
async fn escalation_returns_selected_option() {
    let bus = Arc::new(SignalBus::new());
    let mut escalator = Escalator::new(Pid::new(20), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(Signal {
            target: Pid::new(20),
            kind: SignalKind::Resume,
            payload: json!({
                "hilId": "esc-1",
                "selectedOption": "proceed",
                "guidance": "Go ahead and continue with the plan."
            }),
        })
        .await
        .unwrap();
    });

    let result = escalator
        .escalate(
            "Ambiguous situation",
            "Not sure what to do",
            &[
                ("proceed", "Continue with current plan"),
                ("abort", "Stop execution"),
            ],
            "esc-1",
            Duration::from_secs(2),
        )
        .await
        .unwrap();

    assert_eq!(result.selected_option, "proceed");
    assert_eq!(result.guidance, "Go ahead and continue with the plan.");
}

#[tokio::test]
async fn escalation_guidance_added_to_pending_messages() {
    let bus = Arc::new(SignalBus::new());
    let mut escalator = Escalator::new(Pid::new(21), Arc::clone(&bus));

    let bus2 = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        bus2.send(Signal {
            target: Pid::new(21),
            kind: SignalKind::Resume,
            payload: json!({
                "hilId": "esc-2",
                "selectedOption": "abort",
                "guidance": "Stop immediately — security risk."
            }),
        })
        .await
        .unwrap();
    });

    escalator
        .escalate("danger", "context", &[], "esc-2", Duration::from_secs(2))
        .await
        .unwrap();

    let msgs = escalator.pending_messages();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("Stop immediately — security risk."));
}

#[tokio::test]
async fn escalation_timeout_returns_error() {
    let bus = Arc::new(SignalBus::new());
    let mut escalator = Escalator::new(Pid::new(22), Arc::clone(&bus));
    let result = escalator
        .escalate(
            "situation",
            "context",
            &[],
            "esc-timeout",
            Duration::from_millis(50),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("timed out"));
}

// ---- ApprovalToken tests ----

#[tokio::test]
async fn approval_token_single_use_atomic() {
    let store = ApprovalTokenStore::new();
    let token_id = store.create("hil-id").await;
    assert!(store.consume(&token_id).await.is_ok());
    // Second consume must fail
    let err = store.consume(&token_id).await.unwrap_err();
    assert!(err.to_string().contains("EUSED"));
}

#[tokio::test]
async fn approval_token_not_found_returns_error() {
    let store = ApprovalTokenStore::new();
    let err = store.consume("nonexistent-token").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn approval_token_concurrent_consume_only_one_wins() {
    let store = Arc::new(ApprovalTokenStore::new());
    let token_id = store.create("hil-concurrent").await;

    let s1 = Arc::clone(&store);
    let t1 = token_id.clone();
    let s2 = Arc::clone(&store);
    let t2 = token_id.clone();

    let f1 = tokio::spawn(async move { s1.consume(&t1).await });
    let f2 = tokio::spawn(async move { s2.consume(&t2).await });

    let (r1, r2) = tokio::join!(f1, f2);
    let results = [r1.unwrap(), r2.unwrap()];
    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    let err_count = results.iter().filter(|r| r.is_err()).count();
    assert_eq!(ok_count, 1);
    assert_eq!(err_count, 1);
}
