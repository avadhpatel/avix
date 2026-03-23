use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

use crate::error::AvixError;
use crate::gateway::atp::frame::AtpEvent;
use crate::gateway::atp::types::AtpEventKind;
use crate::gateway::event_bus::AtpEventBus;
use crate::kernel::approval_token::ApprovalTokenStore;
use crate::kernel::hil::{HilRequest, HilState};
use crate::memfs::vfs::MemFs;
use crate::signal::bus::SignalBus;
use crate::signal::kind::{Signal, SignalKind};
use crate::types::{Pid, Role};

pub struct HilManager {
    pending: Arc<RwLock<HashMap<String, HilRequest>>>,
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
    ) -> Arc<Self> {
        Arc::new(Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            approval_store,
            event_bus,
            vfs,
            signal_bus,
            timeout_secs,
        })
    }

    /// Called by RuntimeExecutor when a HIL event is triggered.
    /// Writes the VFS file, pushes hil.request event, starts timeout timer.
    pub async fn open(self: &Arc<Self>, req: HilRequest) -> Result<(), AvixError> {
        let hil_id = req.hil_id.clone();
        let pid = req.pid;
        let session_owner = req.agent_name.clone();

        // 1. Write /proc/<pid>/hil-queue/<hil-id>.yaml
        let vfs_path = req.vfs_path();
        let yaml = serde_yaml::to_string(&req)
            .map_err(|e| AvixError::ConfigParse(format!("yaml serialise: {e}")))?;
        let path = crate::memfs::path::VfsPath::parse(&vfs_path)?;
        self.vfs.write(&path, yaml.into_bytes()).await?;

        // 2. Store in pending map and register approval token
        self.pending
            .write()
            .await
            .insert(hil_id.clone(), req.clone());
        self.approval_store.register(&req.approval_token).await;

        // 3. Push hil.request event to ATP event bus
        let event_body = serde_json::to_value(&req)
            .map_err(|e| AvixError::ConfigParse(format!("json serialise: {e}")))?;
        let event = AtpEvent::new(AtpEventKind::HilRequest, &session_owner, event_body);
        self.event_bus
            .publish(event, Some(session_owner.clone()), Role::User);

        // 4. Start timeout task
        let mgr = Arc::clone(self);
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
        decision: &str,
        resolved_by: &str,
        payload: serde_json::Value,
    ) -> Result<(), AvixError> {
        // 1. Atomically consume the approval token → EUSED if already used
        self.approval_store.consume(approval_token).await?;

        // 2. Update VFS file state
        let session_owner = {
            let guard = self.pending.read().await;
            if let Some(req) = guard.get(hil_id) {
                let mut updated = req.clone();
                updated.state = if decision == "approved" {
                    HilState::Approved
                } else {
                    HilState::Denied
                };
                let name = updated.agent_name.clone();
                let yaml = serde_yaml::to_string(&updated).unwrap_or_default();
                let path = crate::memfs::path::VfsPath::parse(&updated.vfs_path()).ok();
                if let Some(p) = path {
                    self.vfs.write(&p, yaml.into_bytes()).await.ok();
                }
                name
            } else {
                String::new()
            }
        };

        self.pending.write().await.remove(hil_id);

        // 3. Push hil.resolved event
        self.push_resolved(hil_id, decision, resolved_by, &session_owner, &payload)
            .await;

        Ok(())
    }

    async fn timeout_hil(&self, hil_id: &str, pid: Pid) {
        let session_owner = {
            let mut guard = self.pending.write().await;
            if let Some(req) = guard.remove(hil_id) {
                let mut updated = req.clone();
                updated.state = HilState::Timeout;
                let name = updated.agent_name.clone();
                let yaml = serde_yaml::to_string(&updated).unwrap_or_default();
                if let Ok(p) = crate::memfs::path::VfsPath::parse(&updated.vfs_path()) {
                    self.vfs.write(&p, yaml.into_bytes()).await.ok();
                }
                name
            } else {
                // Already resolved before timeout fired
                return;
            }
        };

        // Send SIGRESUME { decision: "timeout" } to agent
        let sig = Signal {
            target: pid,
            kind: SignalKind::Resume,
            payload: serde_json::json!({ "decision": "timeout" }),
        };
        self.signal_bus.send(sig).await.ok();

        self.push_resolved(
            hil_id,
            "timeout",
            "kernel",
            &session_owner,
            &serde_json::json!({}),
        )
        .await;
    }

    async fn push_resolved(
        &self,
        hil_id: &str,
        outcome: &str,
        resolved_by: &str,
        session_owner: &str,
        payload: &serde_json::Value,
    ) {
        let event = AtpEvent::new(
            AtpEventKind::HilResolved,
            session_owner,
            serde_json::json!({
                "hilId": hil_id,
                "outcome": outcome,
                "resolvedBy": resolved_by,
                "resolvedAt": Utc::now(),
                "note": payload.get("note"),
            }),
        );
        self.event_bus
            .publish(event, Some(session_owner.to_string()), Role::User);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::types::AtpEventKind;
    use crate::kernel::hil::{HilRequest, HilState, HilType, HilUrgency};
    use chrono::Utc;
    use serde_json::json;

    fn make_manager_with_timeout(timeout_secs: u64) -> (Arc<HilManager>, Arc<AtpEventBus>) {
        let approval_store = Arc::new(ApprovalTokenStore::new());
        let bus = Arc::new(AtpEventBus::new());
        let vfs = Arc::new(MemFs::new());
        let signal_bus = Arc::new(SignalBus::new());
        let mgr = HilManager::new(
            approval_store,
            Arc::clone(&bus),
            vfs,
            signal_bus,
            timeout_secs,
        );
        (mgr, bus)
    }

    fn make_manager() -> (Arc<HilManager>, Arc<AtpEventBus>) {
        make_manager_with_timeout(3600)
    }

    fn sample_request(hil_id: &str, approval_token: &str) -> HilRequest {
        HilRequest {
            api_version: "avix/v1".into(),
            kind: "HilRequest".into(),
            hil_id: hil_id.into(),
            pid: Pid::new(57),
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
        let (mgr, bus) = make_manager();
        let mut rx = bus.subscribe();
        let req = sample_request("hil-001", "tok-abc");
        mgr.open(req).await.unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilRequest);
    }

    #[tokio::test]
    async fn open_writes_vfs_file() {
        let (mgr, _) = make_manager();
        let req = sample_request("hil-002", "tok-def");
        let vfs_path_str = req.vfs_path();
        mgr.open(req).await.unwrap();
        let path = crate::memfs::path::VfsPath::parse(&vfs_path_str).unwrap();
        assert!(mgr.vfs.exists(&path).await);
    }

    #[tokio::test]
    async fn resolve_approved_pushes_hil_resolved() {
        let (mgr, bus) = make_manager();
        let mut rx = bus.subscribe();
        let req = sample_request("hil-003", "tok-ghi");
        mgr.open(req).await.unwrap();
        // consume hil.request
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        mgr.resolve("hil-003", "tok-ghi", "approved", "alice", json!({}))
            .await
            .unwrap();

        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilResolved);
        assert_eq!(ev.event.body["outcome"], "approved");
    }

    #[tokio::test]
    async fn resolve_denied_pushes_hil_resolved() {
        let (mgr, bus) = make_manager();
        let mut rx = bus.subscribe();
        let req = sample_request("hil-004", "tok-deny");
        mgr.open(req).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        mgr.resolve("hil-004", "tok-deny", "denied", "alice", json!({}))
            .await
            .unwrap();

        let ev = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilResolved);
        assert_eq!(ev.event.body["outcome"], "denied");
    }

    #[tokio::test]
    async fn resolve_same_token_twice_returns_eused() {
        let (mgr, _bus) = make_manager();
        let req = sample_request("hil-005", "tok-jkl");
        mgr.open(req).await.unwrap();
        mgr.resolve("hil-005", "tok-jkl", "approved", "alice", json!({}))
            .await
            .unwrap();
        // second attempt with same token
        let err = mgr
            .resolve("hil-005", "tok-jkl", "approved", "alice", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("EUSED"));
    }

    #[tokio::test]
    async fn timeout_fires_and_pushes_resolved() {
        let (mgr, bus) = make_manager_with_timeout(0);
        let mut rx = bus.subscribe();
        let req = sample_request("hil-006", "tok-mno");
        mgr.open(req).await.unwrap();
        // consume hil.request
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        // Wait for timeout (0-second timeout)
        let ev = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ev.event.event, AtpEventKind::HilResolved);
        assert_eq!(ev.event.body["outcome"], "timeout");
    }

    #[tokio::test]
    async fn timeout_does_not_fire_after_resolve() {
        // Resolve quickly, then ensure the pending entry is gone so timeout is a no-op
        let (mgr, bus) = make_manager_with_timeout(0);
        let mut rx = bus.subscribe();
        let req = sample_request("hil-007", "tok-pqr");
        mgr.open(req).await.unwrap();
        // consume hil.request
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        mgr.resolve("hil-007", "tok-pqr", "approved", "alice", json!({}))
            .await
            .unwrap();
        // consume hil.resolved from resolve()
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();

        // At this point the timeout task may fire, but it should find nothing in pending
        // and return without publishing a second hil.resolved.
        // There should be no additional event within 200ms.
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "no second event expected after early resolve"
        );
    }
}
