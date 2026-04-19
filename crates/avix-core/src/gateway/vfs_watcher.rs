use crate::gateway::atp::frame::AtpEvent;
use tracing::instrument;
use crate::gateway::atp::types::AtpEventKind;
use crate::gateway::event_bus::AtpEventBus;
use crate::types::Role;

/// Called by the VFS layer whenever a watched path is modified.
///
/// Publishes an `fs.changed` ATP event scoped to the session that registered
/// the watch. The event is delivered only to the owning session (or Operator+).
#[instrument(skip(bus))]
pub fn on_vfs_change(bus: &AtpEventBus, path: &str, session_id: &str) {
    let event = AtpEvent::new(
        AtpEventKind::FsChanged,
        session_id,
        serde_json::json!({ "path": path }),
    );
    bus.publish(event, Some(session_id.to_string()), Role::User);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_vfs_change_publishes_fs_changed_event() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        on_vfs_change(&bus, "/users/alice/data.yaml", "sess-001");
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.event.event, AtpEventKind::FsChanged);
        assert_eq!(ev.owner_session.as_deref(), Some("sess-001"));
        assert_eq!(ev.min_role, Role::User);
        assert_eq!(ev.event.body["path"], "/users/alice/data.yaml");
    }

    #[test]
    fn on_vfs_change_is_owner_scoped() {
        let bus = AtpEventBus::new();
        let mut rx = bus.subscribe();
        on_vfs_change(&bus, "/some/path", "sess-abc");
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.owner_session.as_deref(), Some("sess-abc"));
    }
}
