# ATP Gap F — Server-Push Event Bus

> **Spec reference:** §7 Server-Push Events, §5.3 Server→Client Event, §5.4 Subscription
> **Priority:** High
> **Depends on:** ATP Gap A (AtpEvent, AtpEventKind, AtpErrorCode), ATP Gap D (connection state)

---

## Problem

There is no event bus. The spec defines 16 server-push event types that the gateway must
broadcast to connected clients without polling. Events are scoped by role and
subscription filter — clients only receive events they are permitted to see and have
subscribed to. There is also no VFS-change notification pathway into ATP.

---

## What to Build

### 1. `AtpEventBus`

File: `crates/avix-core/src/gateway/event_bus.rs`

The event bus is a broadcast channel. Each WebSocket connection subscribes to it and
filters events by role and subscription list.

```rust
use std::sync::Arc;
use tokio::sync::broadcast;
use crate::gateway::atp::frame::AtpEvent;
use crate::gateway::atp::types::AtpEventKind;
use crate::types::Role;

/// Maximum buffered events per bus before oldest are dropped.
const BUS_CAPACITY: usize = 1024;

/// An envelope carrying an event plus the metadata needed for scoping.
#[derive(Debug, Clone)]
pub struct BusEvent {
    pub event: AtpEvent,
    /// The session/user that "owns" this event (None = system-wide).
    pub owner_session: Option<String>,
    /// Minimum role required to receive this event.
    pub min_role: Role,
}

#[derive(Clone)]
pub struct AtpEventBus {
    tx: broadcast::Sender<BusEvent>,
}

impl AtpEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Publish an event to all subscribed connections.
    pub fn publish(&self, event: AtpEvent, owner_session: Option<String>, min_role: Role) {
        let _ = self.tx.send(BusEvent { event, owner_session, min_role });
    }

    /// Get a receiver for a new connection.
    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.tx.subscribe()
    }
}
```

### 2. Per-connection event filter

File: `crates/avix-core/src/gateway/event_filter.rs`

```rust
use crate::gateway::atp::types::AtpEventKind;
use crate::gateway::event_bus::BusEvent;
use crate::types::Role;

/// Determines whether a connection should receive a given bus event.
pub struct EventFilter {
    pub session_id: String,
    pub role: Role,
    /// Subscribed event kinds. Empty = not yet subscribed (no events delivered).
    /// If contains the sentinel wildcard → all permitted events.
    pub subscribed: Vec<String>,
}

impl EventFilter {
    pub const WILDCARD: &'static str = "*";

    pub fn new(session_id: String, role: Role) -> Self {
        Self { session_id, role, subscribed: vec![] }
    }

    /// Update the subscription list from a `subscribe` frame.
    pub fn set_subscriptions(&mut self, events: Vec<String>) {
        self.subscribed = events;
    }

    /// Returns true if this connection should receive the given bus event.
    pub fn should_receive(&self, ev: &BusEvent) -> bool {
        // Role gate
        if self.role < ev.min_role {
            return false;
        }
        // Ownership gate: if event is owned, only send to the owning session (or Operator+)
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
```

### 3. Event scoping table

Codify spec §7 into constants:

```rust
/// Return the minimum role and owner-scoping rule for each event kind.
pub fn event_scope(kind: &AtpEventKind) -> (Role, bool /* owner-scoped */) {
    match kind {
        AtpEventKind::SessionReady     => (Role::Guest, true),
        AtpEventKind::SessionClosing   => (Role::Guest, true),
        AtpEventKind::TokenExpiring    => (Role::Guest, true),
        AtpEventKind::AgentOutput      => (Role::User,  true),  // Operator+ sees all
        AtpEventKind::AgentStatus      => (Role::User,  true),
        AtpEventKind::AgentToolCall    => (Role::User,  true),
        AtpEventKind::AgentToolResult  => (Role::User,  true),
        AtpEventKind::AgentExit        => (Role::User,  true),
        AtpEventKind::ProcSignal       => (Role::User,  true),
        AtpEventKind::HilRequest       => (Role::User,  true),
        AtpEventKind::HilResolved      => (Role::User,  true),
        AtpEventKind::FsChanged        => (Role::User,  true),  // only watched paths
        AtpEventKind::ToolChanged      => (Role::Guest, false), // system-wide
        AtpEventKind::CronFired        => (Role::User,  true),
        AtpEventKind::SysService       => (Role::Admin, false),
        AtpEventKind::SysAlert         => (Role::Operator, false),
    }
}
```

### 4. Connection event pump task

Added to the connection handler in Gap D. After starting the reader/writer tasks:

```rust
tokio::spawn({
    let mut rx = event_bus.subscribe();
    let filter = Arc::new(RwLock::new(EventFilter::new(session_id.clone(), role)));
    let outbound_tx = outbound_tx.clone();
    async move {
        loop {
            match rx.recv().await {
                Ok(bus_event) => {
                    let f = filter.read().await;
                    if f.should_receive(&bus_event) {
                        if let Ok(s) = serde_json::to_string(&bus_event.event) {
                            let _ = outbound_tx.send(s).await;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("session {} lagged {} events", session_id, n);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
});
```

The `filter` is shared with the subscribe handler so that `AtpFrame::Subscribe` can
update `filter.subscribed`.

### 5. VFS change → `fs.changed` event

File: `crates/avix-core/src/gateway/vfs_watcher.rs`

The VFS already has a change notification mechanism (or will have one after memfs work).
This shim connects it to the ATP event bus:

```rust
/// Called by the VFS whenever a watched path is written.
pub fn on_vfs_change(bus: &AtpEventBus, path: &str, session_id: &str) {
    let event = AtpEvent::new(
        AtpEventKind::FsChanged,
        session_id,
        serde_json::json!({ "path": path }),
    );
    bus.publish(event, Some(session_id.to_string()), Role::User);
}
```

`fs.watch` / `fs.unwatch` commands (from Gap E) register/deregister paths per session.
The VFS watcher table maps `(path, session_id)` → callback.

### 6. Convenience publish helpers

File: `crates/avix-core/src/gateway/event_bus.rs`

```rust
impl AtpEventBus {
    pub fn agent_output(&self, session_id: &str, pid: u32, text: &str) {
        let (min_role, owner_scoped) = event_scope(&AtpEventKind::AgentOutput);
        let event = AtpEvent::new(
            AtpEventKind::AgentOutput,
            session_id,
            serde_json::json!({ "pid": pid, "text": text }),
        );
        let owner = owner_scoped.then(|| session_id.to_string());
        self.publish(event, owner, min_role);
    }

    pub fn agent_exit(&self, session_id: &str, pid: u32, exit_code: i32) { ... }
    pub fn agent_status(&self, session_id: &str, pid: u32, status: &str) { ... }
    pub fn tool_changed(&self, tool_name: &str, change: &str) { ... }
    pub fn sys_alert(&self, message: &str) { ... }
    // ... one helper per event kind
}
```

---

## Tests to Write

File: `crates/avix-core/src/gateway/event_bus.rs` (under `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::atp::types::AtpEventKind;

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
        let ev = make_agent_output_event("sess-001");  // agent.output not in list
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
}
```

---

## Success Criteria

- [ ] `AtpEventBus` uses a broadcast channel; `publish` drops silently if no receivers
- [ ] `EventFilter::should_receive` correctly applies role gate, ownership gate, subscription gate
- [ ] Wildcard subscription `["*"]` receives all permitted events
- [ ] Operator+ overrides ownership gate and sees all sessions' events
- [ ] `sys.service` events blocked for non-Admin
- [ ] `sys.alert` events blocked below Operator
- [ ] `tool.changed` events (system-wide, no owner) delivered to all Guest+ subscribers
- [ ] VFS change notification wired: `fs.watch` → watcher registration → `fs.changed` event
- [ ] All above tests pass; `cargo clippy` zero warnings
