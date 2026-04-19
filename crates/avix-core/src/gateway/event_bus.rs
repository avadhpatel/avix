use tokio::sync::broadcast;
use tracing::instrument;

use crate::gateway::atp::frame::AtpEvent;
use crate::gateway::atp::types::AtpEventKind;
use crate::types::Role;

/// Maximum buffered events per bus before oldest are dropped.
const BUS_CAPACITY: usize = 1024;

/// An envelope carrying an event plus the metadata needed for scoping.
#[derive(Debug, Clone)]
pub struct BusEvent {
    pub event: AtpEvent,
    /// The session that "owns" this event (None = system-wide).
    pub owner_session: Option<String>,
    /// Minimum role required to receive this event.
    pub min_role: Role,
}

/// Return the minimum role and owner-scoping rule for each event kind.
#[instrument(skip_all)]
pub fn event_scope(kind: &AtpEventKind) -> (Role, bool) {
    match kind {
        AtpEventKind::SessionReady => (Role::Guest, true),
        AtpEventKind::SessionClosing => (Role::Guest, true),
        AtpEventKind::SessionAgentAttached => (Role::User, true),
        AtpEventKind::SessionAgentDetached => (Role::User, true),
        AtpEventKind::SessionStatusChanged => (Role::User, true),
        AtpEventKind::TokenExpiring => (Role::Guest, true),
        AtpEventKind::AgentOutput => (Role::User, true),
        AtpEventKind::AgentStatus => (Role::User, true),
        AtpEventKind::AgentToolCall => (Role::User, true),
        AtpEventKind::AgentToolResult => (Role::User, true),
        AtpEventKind::AgentExit => (Role::User, true),
        AtpEventKind::ProcStart => (Role::User, true),
        AtpEventKind::ProcOutput => (Role::User, true),
        AtpEventKind::ProcExit => (Role::User, true),
        AtpEventKind::ProcSignal => (Role::User, true),
        AtpEventKind::HilRequest => (Role::User, true),
        AtpEventKind::HilResolved => (Role::User, true),
        AtpEventKind::FsChanged => (Role::User, true),
        AtpEventKind::ToolChanged => (Role::Guest, false),
        AtpEventKind::CronFired => (Role::User, true),
        AtpEventKind::SysService => (Role::Admin, false),
        AtpEventKind::SysAlert => (Role::Operator, false),
        AtpEventKind::AgentSpawned => (Role::User, true),
        AtpEventKind::AgentOutputChunk => (Role::User, true),
    }
}

/// Per-connection filter — decides whether a BusEvent should reach this connection.
pub struct EventFilter {
    pub session_id: String,
    pub role: Role,
    /// Subscribed event kinds. Empty = not subscribed (nothing delivered).
    /// Contains `"*"` = wildcard (all permitted events delivered).
    pub subscribed: Vec<String>,
}

impl EventFilter {
    pub const WILDCARD: &'static str = "*";

    pub fn new(session_id: String, role: Role) -> Self {
        Self {
            session_id,
            role,
            subscribed: vec![],
        }
    }

    /// Update the subscription list from a `subscribe` frame.
    #[instrument(skip(self))]
    pub fn set_subscriptions(&mut self, events: Vec<String>) {
        self.subscribed = events;
    }

    /// Returns true if this connection should receive the given bus event.
    #[instrument(skip_all)]
    pub fn should_receive(&self, ev: &BusEvent) -> bool {
        // Role gate
        if self.role < ev.min_role {
            return false;
        }
        // Ownership gate: owned events go only to the owning session (or Operator+)
        if let Some(owner) = &ev.owner_session {
            if owner != &self.session_id && self.role < Role::Operator {
                return false;
            }
        }
        // Subscription gate
        if self.subscribed.is_empty() {
            return false;
        }
        if self.subscribed.contains(&Self::WILDCARD.to_string()) {
            return true;
        }
        let event_name = serde_json::to_value(&ev.event.event)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        self.subscribed.contains(&event_name)
    }
}

/// A broadcast bus. Each WebSocket connection subscribes and filters independently.
#[derive(Clone, Debug)]
pub struct AtpEventBus {
    tx: broadcast::Sender<BusEvent>,
}

impl AtpEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Publish an event with scoping metadata.
    #[instrument(skip_all)]
    pub fn publish(&self, event: AtpEvent, owner_session: Option<String>, min_role: Role) {
        let _ = self.tx.send(BusEvent {
            event,
            owner_session,

            min_role,
        });
    }

    /// Subscribe — returns a receiver for this connection.
    #[instrument(skip_all)]
    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.tx.subscribe()
    }

    // ── Convenience publish helpers ─────────────────────────────────────────

    #[instrument(skip(self))]
    pub fn agent_output(&self, atp_session_id: &str, pid: u64, text: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentOutput);
        let ev = AtpEvent::new(
            AtpEventKind::AgentOutput,
            atp_session_id,
            serde_json::json!({ "pid": pid.to_string(), "text": text }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    /// Publish `agent.spawned` so the UI registers the new agent immediately.
    /// `agent_session_id` is the logical conversation UUID (shown in SessionPage).
    #[instrument(skip(self))]
    pub fn agent_spawned(
        &self,
        atp_session_id: &str,
        pid: u64,
        name: &str,
        goal: &str,
        agent_session_id: &str,
    ) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentSpawned);
        let ev = AtpEvent::new(
            AtpEventKind::AgentSpawned,
            atp_session_id,
            serde_json::json!({
                "pid": pid.to_string(),
                "name": name,
                "goal": goal,
                "sessionId": agent_session_id,
            }),
        );
        tracing::debug!(
            atp_session_id,
            pid,
            name,
            agent_session_id,
            "publishing agent.spawned event"
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    /// Publish an incremental token delta from a streaming LLM turn.
    /// `atp_session_id` routes the event to the originating client connection.
    #[instrument(skip(self))]
    pub fn agent_output_chunk(
        &self,
        atp_session_id: &str,
        pid: u64,
        turn_id: &str,
        text_delta: &str,
        seq: u64,
        is_final: bool,
    ) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentOutputChunk);
        let ev = AtpEvent::new(
            AtpEventKind::AgentOutputChunk,
            atp_session_id,
            serde_json::json!({
                "pid": pid.to_string(),
                "turn_id": turn_id,
                "text_delta": text_delta,
                "seq": seq,
                "is_final": is_final,
            }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    /// `atp_session_id` routes the event to the originating client connection.
    #[instrument(skip(self))]
    pub fn agent_exit(&self, atp_session_id: &str, pid: u64, exit_code: i32) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentExit);
        let ev = AtpEvent::new(
            AtpEventKind::AgentExit,
            atp_session_id,
            serde_json::json!({ "pid": pid.to_string(), "exitCode": exit_code }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    /// `atp_session_id` routes the event to the originating client connection.
    #[instrument(skip(self))]
    pub fn agent_status(&self, atp_session_id: &str, pid: u64, status: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentStatus);
        let ev = AtpEvent::new(
            AtpEventKind::AgentStatus,
            atp_session_id,
            serde_json::json!({ "pid": pid.to_string(), "status": status }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    #[instrument(skip(self))]
    pub fn tool_changed(&self, tool_name: &str, change: &str) {
        let (min_role, _owner_scoped) = event_scope(&AtpEventKind::ToolChanged);
        let ev = AtpEvent::new(
            AtpEventKind::ToolChanged,
            "",
            serde_json::json!({ "tool": tool_name, "change": change }),
        );
        self.publish(ev, None, min_role);
    }

    #[instrument(skip(self))]
    pub fn sys_service(&self, service: &str, status: &str) {
        let (min_role, _owner_scoped) = event_scope(&AtpEventKind::SysService);
        let ev = AtpEvent::new(
            AtpEventKind::SysService,
            "",
            serde_json::json!({ "service": service, "status": status }),
        );
        self.publish(ev, None, min_role);
    }

    #[instrument(skip(self))]
    pub fn sys_alert(&self, message: &str) {
        let (min_role, _owner_scoped) = event_scope(&AtpEventKind::SysAlert);
        let ev = AtpEvent::new(
            AtpEventKind::SysAlert,
            "",
            serde_json::json!({ "message": message }),
        );
        self.publish(ev, None, min_role);
    }

    #[instrument(skip(self))]
    pub fn fs_changed(&self, atp_session_id: &str, path: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::FsChanged);
        let ev = AtpEvent::new(
            AtpEventKind::FsChanged,
            atp_session_id,
            serde_json::json!({ "path": path }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    #[instrument(skip(self))]
    pub fn hil_request(&self, atp_session_id: &str, hil_id: &str, kind: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::HilRequest);
        let ev = AtpEvent::new(
            AtpEventKind::HilRequest,
            atp_session_id,
            serde_json::json!({ "hilId": hil_id, "kind": kind }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);

    }

    #[instrument(skip(self))]
    pub fn hil_resolved(&self, atp_session_id: &str, hil_id: &str, outcome: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::HilResolved);
        let ev = AtpEvent::new(
            AtpEventKind::HilResolved,
            atp_session_id,
            serde_json::json!({ "hilId": hil_id, "outcome": outcome }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    #[instrument(skip(self))]
    pub fn agent_tool_call(
        &self,
        atp_session_id: &str,
        pid: u64,
        call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentToolCall);
        let ev = AtpEvent::new(
            AtpEventKind::AgentToolCall,
            atp_session_id,
            serde_json::json!({ "pid": pid.to_string(), "callId": call_id, "tool": tool_name, "args": args }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }

    #[instrument(skip(self))]
    pub fn agent_tool_result(
        &self,
        atp_session_id: &str,
        pid: u64,
        call_id: &str,
        tool_name: &str,
        result: &str,
    ) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentToolResult);
        let ev = AtpEvent::new(
            AtpEventKind::AgentToolResult,
            atp_session_id,
            serde_json::json!({ "pid": pid.to_string(), "callId": call_id, "tool": tool_name, "result": result }),
        );
        self.publish(ev, owner_scoped.then(|| atp_session_id.to_string()), min_role);
    }
}

impl Default for AtpEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent_output_event(session: &str) -> BusEvent {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentOutput);
        BusEvent {
            event: AtpEvent::new(AtpEventKind::AgentOutput, session, serde_json::json!({})),
            owner_session: owner_scoped.then(|| session.to_string()),
            min_role,
        }
    }

    #[tokio::test]
    async fn published_event_received_by_subscriber() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(
            AtpEvent::new(AtpEventKind::ToolChanged, "sess-001", serde_json::json!({})),
            None,
            Role::Guest,
        );
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.event.event, AtpEventKind::ToolChanged);
    }

    #[test]
    fn filter_blocks_guest_for_user_only_event() {
        let mut f = EventFilter::new("sess-001".into(), Role::Guest);
        f.set_subscriptions(vec!["*".into()]);
        let ev = make_agent_output_event("sess-001");
        assert!(!f.should_receive(&ev));
    }

    #[test]
    fn filter_allows_user_for_own_agent_output() {
        let mut f = EventFilter::new("sess-001".into(), Role::User);
        f.set_subscriptions(vec!["*".into()]);
        let ev = make_agent_output_event("sess-001");
        assert!(f.should_receive(&ev));
    }

    #[test]
    fn filter_blocks_user_for_other_session_event() {
        let mut f = EventFilter::new("sess-001".into(), Role::User);
        f.set_subscriptions(vec!["*".into()]);
        let ev = make_agent_output_event("sess-002");
        assert!(!f.should_receive(&ev));
    }

    #[test]
    fn operator_receives_any_session_event() {
        let mut f = EventFilter::new("sess-001".into(), Role::Operator);
        f.set_subscriptions(vec!["*".into()]);
        let ev = make_agent_output_event("sess-002");
        assert!(f.should_receive(&ev));
    }

    #[test]
    fn filter_blocks_unsubscribed_event() {
        let mut f = EventFilter::new("sess-001".into(), Role::User);
        f.set_subscriptions(vec!["agent.exit".into()]);
        let ev = make_agent_output_event("sess-001"); // agent.output not in list
        assert!(!f.should_receive(&ev));
    }

    #[test]
    fn wildcard_subscription_allows_all_permitted() {
        let mut f = EventFilter::new("sess-001".into(), Role::User);
        f.set_subscriptions(vec!["*".into()]);
        let ev = make_agent_output_event("sess-001");
        assert!(f.should_receive(&ev));
    }

    #[test]
    fn no_subscriptions_blocks_all() {
        let f = EventFilter::new("sess-001".into(), Role::Admin);
        // no set_subscriptions call
        let ev = make_agent_output_event("sess-001");
        assert!(!f.should_receive(&ev));
    }

    #[test]
    fn admin_receives_sys_service_events() {
        let mut f = EventFilter::new("sess-001".into(), Role::Admin);
        f.set_subscriptions(vec!["*".into()]);
        let ev = BusEvent {
            event: AtpEvent::new(AtpEventKind::SysService, "sess-001", serde_json::json!({})),
            owner_session: None,
            min_role: Role::Admin,
        };
        assert!(f.should_receive(&ev));
    }

    #[test]
    fn operator_blocked_from_sys_service_events() {
        let mut f = EventFilter::new("sess-001".into(), Role::Operator);
        f.set_subscriptions(vec!["*".into()]);
        let ev = BusEvent {
            event: AtpEvent::new(AtpEventKind::SysService, "sess-001", serde_json::json!({})),
            owner_session: None,
            min_role: Role::Admin,
        };
        assert!(!f.should_receive(&ev));
    }

    #[test]
    fn tool_changed_event_is_not_owner_scoped() {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::ToolChanged);
        assert_eq!(min_role, Role::Guest);
        assert!(!owner_scoped);
    }

    #[test]
    fn agent_output_event_is_owner_scoped() {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentOutput);
        assert_eq!(min_role, Role::User);
        assert!(owner_scoped);
    }

    #[test]
    fn sys_alert_requires_operator() {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::SysAlert);
        assert_eq!(min_role, Role::Operator);
        assert!(!owner_scoped);
    }

    #[test]
    fn sys_service_requires_admin() {
        let (min_role, _) = event_scope(&AtpEventKind::SysService);
        assert_eq!(min_role, Role::Admin);
    }

    #[test]
    fn specific_subscription_receives_matching_event() {
        let mut f = EventFilter::new("sess-001".into(), Role::User);
        f.set_subscriptions(vec!["agent.output".into()]);
        let ev = make_agent_output_event("sess-001");
        assert!(f.should_receive(&ev));
    }

    #[test]
    fn convenience_agent_output_publishes_bus_event() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        bus.agent_output("sess-x", 42, "hello");
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.event.event, AtpEventKind::AgentOutput);
        assert_eq!(ev.owner_session.as_deref(), Some("sess-x"));
        assert_eq!(ev.min_role, Role::User);
    }

    #[test]
    fn convenience_tool_changed_has_no_owner() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        bus.tool_changed("fs/read", "updated");
        let ev = rx.try_recv().unwrap();
        assert!(ev.owner_session.is_none());
        assert_eq!(ev.min_role, Role::Guest);
    }

    #[test]
    fn convenience_sys_service_publishes_event() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        bus.sys_service("router.svc", "running");
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.event.event, AtpEventKind::SysService);
        assert!(ev.owner_session.is_none());
        assert_eq!(ev.min_role, Role::Admin);
    }
}
