# ATP Gap G — HIL ATP Integration

> **Spec reference:** §7.1 Human-in-the-Loop (HIL) — Full Specification
> **Priority:** High
> **Depends on:** ATP Gap A, Gap B, Gap D (transport), Gap E (signal.send), Gap F (event bus), `kernel/approval_token.rs`

---

## Problem

The `ApprovalTokenStore` (atomic single-use tokens) and the HIL executor types already
exist. What is missing is the ATP-facing plumbing:

1. `hil.request` events are never pushed to ATP clients
2. `/proc/<pid>/hil-queue/<hil-id>.yaml` is never written
3. `signal.send SIGRESUME` with an `approvalToken` payload is not handled
4. HIL timeout auto-deny is not implemented
5. `hil.resolved` events are never pushed
6. `EUSED` error code is not surfaced through ATP

---

## What to Build

### 1. `HilRequest` VFS schema

File: `crates/avix-core/src/kernel/hil.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::types::Pid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HilType {
    ToolCallApproval,
    CapabilityUpgrade,
    Escalation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HilState {
    Pending,
    Approved,
    Denied,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilRequest {
    pub api_version: String,   // "avix/v1"
    pub kind: String,          // "HilRequest"
    pub hil_id: String,
    pub pid: Pid,
    pub agent_name: String,
    pub hil_type: HilType,
    pub tool: Option<String>,          // tool_call_approval + capability_upgrade
    pub args: Option<serde_json::Value>, // tool_call_approval only
    pub reason: Option<String>,
    pub context: Option<String>,       // escalation
    pub options: Option<Vec<HilOption>>, // escalation
    pub urgency: HilUrgency,
    pub approval_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub state: HilState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HilUrgency { Low, Normal, High }

impl HilRequest {
    /// The VFS path where this request is written.
    pub fn vfs_path(&self) -> String {
        format!("/proc/{}/hil-queue/{}.yaml", self.pid, self.hil_id)
    }
}
```

### 2. `HilManager` — orchestrates the HIL lifecycle

File: `crates/avix-core/src/kernel/hil_manager.rs`

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};
use crate::kernel::approval_token::ApprovalTokenStore;
use crate::kernel::hil::{HilRequest, HilState};
use crate::gateway::event_bus::AtpEventBus;
use crate::gateway::atp::frame::AtpEvent;
use crate::gateway::atp::types::AtpEventKind;
use crate::memfs::vfs::MemFs;
use crate::types::Pid;
use crate::signal::bus::SignalBus;

pub struct HilManager {
    pending: Arc<RwLock<HashMap<String, HilRequest>>>,  // hil_id → request
    approval_store: Arc<ApprovalTokenStore>,
    event_bus: Arc<AtpEventBus>,
    vfs: Arc<MemFs>,
    signal_bus: Arc<SignalBus>,
    timeout_secs: u64,
}

impl HilManager {
    pub fn new(
        approval_store: Arc<ApprovalTokenStore>,
        event_bus: Arc<AtpEventBus>,
        vfs: Arc<MemFs>,
        signal_bus: Arc<SignalBus>,
        timeout_secs: u64,
    ) -> Self { ... }

    /// Called by RuntimeExecutor when a HIL event is triggered.
    /// Writes the VFS file, pushes hil.request event, starts timeout timer.
    pub async fn open(&self, req: HilRequest) -> Result<(), AvixError> {
        let hil_id = req.hil_id.clone();
        let pid = req.pid;
        let session_owner = /* look up from process table */;

        // 1. Write /proc/<pid>/hil-queue/<hil-id>.yaml
        let yaml = serde_yaml::to_string(&req)?;
        self.vfs.write(req.vfs_path(), yaml.into_bytes()).await?;

        // 2. Store in pending map
        self.pending.write().await.insert(hil_id.clone(), req.clone());

        // 3. Push hil.request event to ATP event bus
        let event = AtpEvent::new(
            AtpEventKind::HilRequest,
            &session_owner,
            serde_json::to_value(&req)?,
        );
        self.event_bus.publish(event, Some(session_owner.clone()), crate::types::Role::User);

        // 4. Start timeout task
        let mgr = self.clone_arc();
        tokio::spawn(async move {
            sleep(Duration::from_secs(mgr.timeout_secs)).await;
            mgr.timeout_hil(&hil_id, pid).await;
        });

        Ok(())
    }

    /// Called when a SIGRESUME with approvalToken arrives.
    pub async fn resolve(
        &self,
        hil_id: &str,
        approval_token: &str,
        decision: &str,           // "approved" | "denied"
        resolved_by: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
        // 1. Atomically consume the approval token → EUSED if already used
        self.approval_store.consume(approval_token).await?;

        // 2. Update VFS file state
        if let Some(req) = self.pending.read().await.get(hil_id) {
            let mut updated = req.clone();
            updated.state = if decision == "approved" {
                HilState::Approved
            } else {
                HilState::Denied
            };
            let yaml = serde_yaml::to_string(&updated)?;
            self.vfs.write(updated.vfs_path(), yaml.into_bytes()).await.ok();
        }

        self.pending.write().await.remove(hil_id);

        // 3. Push hil.resolved event
        self.push_resolved(hil_id, decision, resolved_by, &payload).await;

        Ok(())
    }

    async fn timeout_hil(&self, hil_id: &str, pid: Pid) {
        let mut guard = self.pending.write().await;
        if let Some(req) = guard.remove(hil_id) {
            // Update VFS
            let mut updated = req.clone();
            updated.state = HilState::Timeout;
            let yaml = serde_yaml::to_string(&updated).unwrap_or_default();
            self.vfs.write(updated.vfs_path(), yaml.into_bytes()).await.ok();
            drop(guard);

            // Send SIGRESUME { decision: "timeout" } to agent
            self.signal_bus
                .send(pid, crate::signal::kind::SignalKind::SigResume,
                      serde_json::json!({ "decision": "timeout" }))
                .await
                .ok();

            self.push_resolved(hil_id, "timeout", "kernel", &serde_json::json!({})).await;
        }
    }

    async fn push_resolved(&self, hil_id: &str, outcome: &str, resolved_by: &str,
                            payload: &serde_json::Value) {
        let session_owner = /* look up or best-effort */;
        let event = AtpEvent::new(
            AtpEventKind::HilResolved,
            &session_owner,
            serde_json::json!({
                "hilId": hil_id,
                "outcome": outcome,
                "resolvedBy": resolved_by,
                "resolvedAt": chrono::Utc::now(),
                "note": payload.get("note"),
            }),
        );
        self.event_bus.publish(event, Some(session_owner), crate::types::Role::User);
    }
}
```

### 3. Wire `signal.send` SIGRESUME → `HilManager::resolve`

In the `signal` domain handler (Gap E `handlers/signal.rs`), add special-casing for
SIGRESUME with `approvalToken`:

```rust
"send" => {
    let signal = body["signal"].as_str().unwrap_or("");
    let target_pid = body["target"].as_u64().unwrap_or(0) as u32;
    let payload = &body["payload"];

    if signal == "SIGRESUME" {
        if let Some(approval_token) = payload.get("approvalToken").and_then(|v| v.as_str()) {
            let hil_id = /* look up from pending by token or require hil_id in payload */;
            let decision = payload["decision"].as_str().unwrap_or("denied");
            return match hil_manager.resolve(
                &hil_id, approval_token, decision,
                &cmd.caller_identity, payload.clone(),
            ).await {
                Ok(_) => AtpReply::ok(cmd.cmd.id, json!({ "ok": true })),
                Err(e) if e.to_string().contains("EUSED") =>
                    AtpReply::err(cmd.cmd.id, AtpError::new(AtpErrorCode::Eused,
                        "approval token already consumed")),
                Err(e) => AtpReply::err(cmd.cmd.id, ipc_err_to_atp(e)),
            };
        }
    }
    // normal signal flow...
}
```

### 4. Wire `RuntimeExecutor` HIL triggers → `HilManager::open`

In `crates/avix-core/src/executor/hil/mod.rs`, the three HIL scenarios call
`HilManager::open` instead of (or in addition to) their current local logic.

This is the IPC bridge point: RuntimeExecutor already calls the kernel; the kernel now
calls `HilManager::open` which drives the ATP side.

---

## Tests to Write

File: `crates/avix-core/src/kernel/hil_manager.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    async fn make_manager() -> (HilManager, Arc<AtpEventBus>) {
        // use test doubles for VFS, signal bus
        ...
    }

    fn sample_request(hil_id: &str, approval_token: &str) -> HilRequest {
        HilRequest {
            api_version: "avix/v1".into(),
            kind: "HilRequest".into(),
            hil_id: hil_id.into(),
            pid: crate::types::Pid::new(57),
            agent_name: "researcher".into(),
            hil_type: HilType::ToolCallApproval,
            tool: Some("send_email".into()),
            args: None,
            reason: Some("wants to send email".into()),
            context: None,
            options: None,
            urgency: HilUrgency::Normal,
            approval_token: approval_token.into(),
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::minutes(10),
            state: HilState::Pending,
        }
    }

    #[tokio::test]
    async fn open_pushes_hil_request_event() {
        let (mgr, bus) = make_manager().await;
        let mut rx = bus.subscribe();
        let req = sample_request("hil-001", "tok-abc");
        mgr.open(req).await.unwrap();
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilRequest);
    }

    #[tokio::test]
    async fn open_writes_vfs_file() {
        let (mgr, _) = make_manager().await;
        let req = sample_request("hil-002", "tok-def");
        let path = req.vfs_path();
        mgr.open(req).await.unwrap();
        // VFS should have the file
        assert!(mgr.vfs.exists(&path).await);
    }

    #[tokio::test]
    async fn resolve_approved_pushes_hil_resolved() {
        let (mgr, bus) = make_manager().await;
        let mut rx = bus.subscribe();
        let req = sample_request("hil-003", "tok-ghi");
        mgr.open(req).await.unwrap();
        rx.recv().await.unwrap(); // consume hil.request

        mgr.resolve("hil-003", "tok-ghi", "approved", "alice", json!({}))
            .await
            .unwrap();

        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilResolved);
        assert_eq!(ev.event.body["outcome"], "approved");
    }

    #[tokio::test]
    async fn resolve_same_token_twice_returns_eused() {
        let (mgr, _bus) = make_manager().await;
        let req = sample_request("hil-004", "tok-jkl");
        mgr.open(req).await.unwrap();
        mgr.resolve("hil-004", "tok-jkl", "approved", "alice", json!({}))
            .await
            .unwrap();
        // second attempt with same token
        let err = mgr
            .resolve("hil-004", "tok-jkl", "approved", "alice", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("EUSED"));
    }

    #[tokio::test]
    async fn timeout_sends_sigresume_with_timeout_decision() {
        // Use a very short timeout (100ms) for test
        let (mgr, bus) = make_manager_with_timeout(0).await;
        let mut rx = bus.subscribe();
        let req = sample_request("hil-005", "tok-mno");
        mgr.open(req).await.unwrap();
        rx.recv().await.unwrap(); // hil.request

        // Wait for timeout
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let ev = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            rx.recv(),
        ).await.unwrap().unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilResolved);
        assert_eq!(ev.event.body["outcome"], "timeout");
    }

    #[tokio::test]
    async fn vfs_path_format_is_correct() {
        let req = sample_request("hil-abc", "tok");
        assert_eq!(req.vfs_path(), "/proc/57/hil-queue/hil-abc.yaml");
    }
}
```

---

## Success Criteria

- [ ] `HilRequest` serialises to YAML matching the spec schema
- [ ] `HilManager::open` writes `/proc/<pid>/hil-queue/<hil-id>.yaml` to VFS
- [ ] `HilManager::open` pushes `hil.request` event to ATP event bus
- [ ] `HilManager::open` starts a timeout timer
- [ ] On timeout: SIGRESUME `{ decision: "timeout" }` sent to agent; `hil.resolved` event pushed
- [ ] `HilManager::resolve` atomically consumes approval token (delegates to `ApprovalTokenStore`)
- [ ] Second resolve call returns `EUSED` → surfaced as `AtpErrorCode::Eused` through ATP
- [ ] `SIGRESUME` with `approvalToken` in body routes through `HilManager::resolve`
- [ ] `hil.resolved` event pushed on approve, deny, and timeout
- [ ] All above tests pass; `cargo clippy` zero warnings
