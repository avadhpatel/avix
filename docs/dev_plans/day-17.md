# Day 17 — RuntimeExecutor: Human-in-Loop (Three Scenarios)

> **Goal:** Implement all three HIL scenarios: tool_call_approval (SIGPAUSE → SIGRESUME), capability_upgrade (session-scope and once-scope), and escalation (guidance injected into Block 4). Implement ApprovalToken single-use atomicity in the kernel side.

---

## Pre-flight: Verify Day 16

```bash
cargo test --workspace
grep -r "TurnAction"       crates/avix-core/src/
grep -r "validate_tool_call" crates/avix-core/src/
grep -r "interpret_stop_reason" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Module Setup

Add to `src/executor/`:

```
src/executor/hil/
├── mod.rs
├── approval.rs     ← tool_call_approval flow
├── cap_upgrade.rs  ← capability upgrade flow
└── escalation.rs   ← escalation + guidance injection
```

Add to `src/`:

```
src/kernel/
├── mod.rs
└── approval_token.rs  ← ApprovalToken single-use atomicity
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/hil.rs`:

```rust
use std::time::Duration;
use avix_core::executor::hil::*;
use avix_core::signal::{SignalBus, SignalKind};
use avix_core::types::{Pid, token::CapabilityToken};
use std::sync::Arc;
use serde_json::json;

// ── Scenario 1: Tool Call Approval ───────────────────────────────────────────

#[tokio::test]
async fn tool_call_approval_approved() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(57), Arc::clone(&bus));

    // Send SIGRESUME with approved decision after short delay
    let b = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target:  Pid::new(57),
            kind:    SignalKind::Resume,
            payload: json!({"hilId": "hil-001", "decision": "approved", "note": "Go ahead"}),
        }).await.unwrap();
    });

    let result = approver.await_approval("hil-001", Duration::from_millis(200)).await.unwrap();
    assert!(result.approved);
    assert_eq!(result.note.as_deref(), Some("Go ahead"));
}

#[tokio::test]
async fn tool_call_approval_denied() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(57), Arc::clone(&bus));

    let b = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target: Pid::new(57), kind: SignalKind::Resume,
            payload: json!({"hilId": "hil-001", "decision": "denied", "reason": "Not allowed"}),
        }).await.unwrap();
    });

    let result = approver.await_approval("hil-001", Duration::from_millis(200)).await.unwrap();
    assert!(!result.approved);
    assert_eq!(result.denial_reason.as_deref(), Some("Not allowed"));
}

#[tokio::test]
async fn tool_call_approval_timeout_treated_as_denied() {
    let bus = Arc::new(SignalBus::new());
    let approver = HilApprover::new(Pid::new(57), Arc::clone(&bus));

    let b = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target: Pid::new(57), kind: SignalKind::Resume,
            payload: json!({"hilId": "hil-001", "decision": "timeout"}),
        }).await.unwrap();
    });

    let result = approver.await_approval("hil-001", Duration::from_millis(200)).await.unwrap();
    assert!(!result.approved);
}

// ── Scenario 2: Capability Upgrade ───────────────────────────────────────────

#[tokio::test]
async fn capability_upgrade_session_scope_replaces_token() {
    let bus = Arc::new(SignalBus::new());
    let initial_token = token_without_tool("send_email");
    let new_token = token_with_tool("send_email");

    let mut upgrader = CapabilityUpgrader::new(Pid::new(57), initial_token.clone(), Arc::clone(&bus));

    let b = Arc::clone(&bus);
    let nt = new_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target: Pid::new(57), kind: SignalKind::Resume,
            payload: json!({
                "hilId": "hil-002",
                "decision": "approved",
                "scope": "session",
                "new_capability_token": serde_json::to_value(&nt).unwrap()
            }),
        }).await.unwrap();
    });

    upgrader.request_tool("send_email", "Need to notify", "hil-002", Duration::from_millis(200)).await.unwrap();
    assert!(upgrader.current_token().has_tool("send_email"));
}

#[tokio::test]
async fn capability_upgrade_denied_leaves_token_unchanged() {
    let bus = Arc::new(SignalBus::new());
    let initial_token = token_without_tool("send_email");
    let original_sig = initial_token.signature.clone();
    let mut upgrader = CapabilityUpgrader::new(Pid::new(57), initial_token, Arc::clone(&bus));

    let b = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target: Pid::new(57), kind: SignalKind::Resume,
            payload: json!({"hilId": "hil-002", "decision": "denied"}),
        }).await.unwrap();
    });

    let result = upgrader.request_tool("send_email", "reason", "hil-002", Duration::from_millis(200)).await;
    assert!(result.is_err());
    assert_eq!(upgrader.current_token().signature, original_sig); // token unchanged
}

// ── Scenario 3: Escalation ────────────────────────────────────────────────────

#[tokio::test]
async fn escalation_guidance_injected_as_block4_message() {
    let bus = Arc::new(SignalBus::new());
    let mut escalator = Escalator::new(Pid::new(57), Arc::clone(&bus));

    let b = Arc::clone(&bus);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        b.send(avix_core::signal::Signal {
            target: Pid::new(57), kind: SignalKind::Resume,
            payload: json!({
                "hilId": "hil-003",
                "decision": "approved",
                "selectedOption": "exclude",
                "guidance": "Exclude salary data. Focus on revenue only."
            }),
        }).await.unwrap();
    });

    let guidance = escalator.escalate(
        "Found PII",
        "Q3 research",
        &[("exclude", "Exclude entirely")],
        "hil-003",
        Duration::from_millis(200),
    ).await.unwrap();

    assert_eq!(guidance.selected_option, "exclude");
    assert!(escalator.pending_messages().iter().any(|m| m.contains("Exclude salary data")));
}

// ── ApprovalToken single-use atomicity ────────────────────────────────────────

#[tokio::test]
async fn approval_token_single_use_enforced() {
    use avix_core::kernel::approval_token::ApprovalTokenStore;

    let store = ApprovalTokenStore::new();
    let token_id = store.create("hil-001").await;

    assert!(store.consume(&token_id).await.is_ok());
    // Second use fails
    let err = store.consume(&token_id).await.unwrap_err();
    assert!(err.to_string().contains("already used") || err.to_string().contains("EUSED"));
}

#[tokio::test]
async fn approval_token_concurrent_consumers_only_one_wins() {
    use avix_core::kernel::approval_token::ApprovalTokenStore;
    use std::sync::Arc;

    let store = Arc::new(ApprovalTokenStore::new());
    let token_id = store.create("hil-race").await;

    let mut handles = Vec::new();
    for _ in 0..5 {
        let s = Arc::clone(&store);
        let t = token_id.clone();
        handles.push(tokio::spawn(async move {
            s.consume(&t).await.is_ok()
        }));
    }

    let results: Vec<bool> = futures::future::join_all(handles).await
        .into_iter().map(|r| r.unwrap()).collect();

    let wins = results.iter().filter(|&&ok| ok).count();
    assert_eq!(wins, 1, "Exactly one consumer should win, got {wins}");
}

// helpers
fn token_with_tool(tool: &str) -> CapabilityToken {
    CapabilityToken { granted_tools: vec![tool.to_string()], signature: "sig-with".into() }
}
fn token_without_tool(tool: &str) -> CapabilityToken {
    CapabilityToken { granted_tools: vec![], signature: "sig-without".into() }
}
```

---

## Step 3 — Implement

`HilApprover`: subscribes to `SignalBus` for the target PID, waits for `SIGRESUME` with matching `hilId`, times out and returns denied.

`CapabilityUpgrader`: wraps `HilApprover`, on approval with `scope: session` replaces held token with `new_capability_token` from payload.

`Escalator`: similar pattern, extracts `guidance` and `selectedOption` from payload, appends `"[Human guidance]: <guidance>"` to `pending_messages`.

`ApprovalTokenStore`: `HashMap<token_id, AtomicBool>` under `RwLock`. `consume` uses `compare_exchange` to ensure only one caller wins.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: 25+ HIL tests pass
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Commit

```bash
git add -A
git commit -m "day-17: HIL — approval/denial/timeout, capability upgrade, escalation, ApprovalToken"
```

## Success Criteria

- [ ] Tool approval: approved / denied / timeout all produce correct outcome
- [ ] Capability upgrade `session` scope replaces held token
- [ ] Capability upgrade denied → token signature unchanged
- [ ] Escalation guidance appears in Block 4 pending messages
- [ ] `ApprovalToken` single-use: second consume returns `EUSED`
- [ ] Concurrent consumers: exactly 1 wins (race test)
- [ ] 25+ tests pass, 0 clippy warnings

---
---

