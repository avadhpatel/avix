use super::entry::ToolEntry;
use super::events::ToolChangedEvent;
use crate::error::AvixError;
use crate::gateway::event_bus::AtpEventBus;
use crate::syscall::SyscallRegistry;
use crate::types::tool::{ToolName, ToolState, ToolVisibility};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tracing::instrument;

const EVENT_CAPACITY: usize = 64;

#[derive(Debug)]
pub struct ToolCallGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Debug)]
struct ToolRecord {
    entry: ToolEntry,
    semaphore: Arc<Semaphore>,
}

#[derive(Debug)]
pub struct ToolRegistry {
    inner: Arc<RwLock<HashMap<String, ToolRecord>>>,
    events: broadcast::Sender<ToolChangedEvent>,
}

#[derive(Debug)]
pub struct EventReceiver(broadcast::Receiver<ToolChangedEvent>);

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<ToolChangedEvent> {
        self.0.recv().await.ok()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            events: tx,
        }
    }

    pub fn new_with_events() -> (Self, EventReceiver) {
        let (tx, rx) = broadcast::channel(EVENT_CAPACITY);
        let reg = Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            events: tx,
        };
        (reg, EventReceiver(rx))
    }

    pub async fn start_atp_bridge(self: Arc<Self>, bus: Arc<AtpEventBus>) {
        let mut rx = self.events.subscribe();
        tokio::spawn(async move {
            while let Ok(evt) = rx.recv().await {
                for tool in &evt.tools {
                    bus.tool_changed(tool, &evt.op);
                }
            }
            tracing::debug!("tool registry ATP bridge terminated");
        });
    }

    pub async fn add(&self, _owner: &str, entries: Vec<ToolEntry>) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        let mut names = Vec::new();
        for entry in entries {
            let name = entry.name.as_str().to_string();
            names.push(name.clone());
            guard.insert(
                name,
                ToolRecord {
                    entry,
                    semaphore: Arc::new(Semaphore::new(tokio::sync::Semaphore::MAX_PERMITS)),
                },
            );
        }
        let _ = self.events.send(ToolChangedEvent {
            op: "added".into(),
            tools: names,
        });
        Ok(())
    }

    pub async fn add_kernel_syscalls(&self, sysreg: &SyscallRegistry) -> Result<(), AvixError> {
        let mut entries = Vec::new();
        for syscall in sysreg.list() {
            let name = ToolName::parse(&syscall.name)
                .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
            let descriptor = serde_json::json!({
                "name": syscall.name,
                "description": syscall.description,
                "short": syscall.short,
                "detailed": syscall.detailed,
                "domain": syscall.domain,
                "handler_signature": syscall.handler_signature,
                "capabilities_required": syscall.capabilities_required,
            });
            entries.push(
                ToolEntry::new(
                    name,
                    "kernel".to_string(),
                    ToolState::Available,
                    ToolVisibility::All,
                    descriptor,
                )
                .with_capabilities(syscall.capabilities_required.clone()),
            );
        }
        self.add("kernel", entries).await
    }

    pub async fn lookup(&self, name: &str) -> Result<ToolEntry, AvixError> {
        self.inner
            .read()
            .await
            .get(name)
            .map(|r| r.entry.clone())
            .ok_or_else(|| AvixError::ConfigParse(format!("tool not found: {name}")))
    }

    pub async fn lookup_for_user(&self, name: &str, user: &str) -> Result<ToolEntry, AvixError> {
        let entry = self.lookup(name).await?;
        match &entry.visibility {
            ToolVisibility::All => Ok(entry),
            ToolVisibility::User(u) if u == user => Ok(entry),
            _ => Err(AvixError::CapabilityDenied(format!(
                "{name} not visible to {user}"
            ))),
        }
    }

    pub async fn remove(
        &self,
        _owner: &str,
        names: &[&str],
        reason: &str,
        drain: bool,
    ) -> Result<(), AvixError> {
        if drain {
            // Phase 1: mark unavailable so the router rejects new calls immediately.
            {
                let mut guard = self.inner.write().await;
                for name in names {
                    if let Some(rec) = guard.get_mut(*name) {
                        rec.entry.state = ToolState::Unavailable;
                    }
                }
            }
            // Phase 2: wait for in-flight calls to complete (all permits returned).
            for name in names {
                let sem = {
                    let guard = self.inner.read().await;
                    guard.get(*name).map(|r| Arc::clone(&r.semaphore))
                };
                if let Some(sem) = sem {
                    loop {
                        if sem.available_permits() == tokio::sync::Semaphore::MAX_PERMITS {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                }
            }
        }

        // Phase 3: remove entries.
        let mut guard = self.inner.write().await;
        let removed: Vec<String> = names
            .iter()
            .filter(|n| guard.remove(**n).is_some())
            .map(|n| n.to_string())
            .collect();
        if !removed.is_empty() {
            let _ = self.events.send(ToolChangedEvent {
                op: format!("removed: {reason}"),
                tools: removed,
            });
        }
        Ok(())
    }

    pub async fn acquire(&self, name: &str) -> Result<ToolCallGuard, AvixError> {
        let sem = {
            let guard = self.inner.read().await;
            guard
                .get(name)
                .map(|r| Arc::clone(&r.semaphore))
                .ok_or_else(|| AvixError::ConfigParse(format!("tool not found: {name}")))?
        };
        let permit = sem
            .acquire_owned()
            .await
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        Ok(ToolCallGuard { _permit: permit })
    }

    pub async fn set_state(&self, name: &str, state: ToolState) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        guard
            .get_mut(name)
            .ok_or_else(|| AvixError::ConfigParse(format!("tool not found: {name}")))?
            .entry
            .state = state;
        Ok(())
    }

    pub async fn tool_count(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Return all tool entries with full details (for /tools/ VFS population)
    pub async fn get_all_entries(&self) -> Vec<ToolEntry> {
        self.inner
            .read()
            .await
            .values()
            .map(|rec| rec.entry.clone())
            .collect()
    }

    /// Return a snapshot of every registered tool with its name, namespace, description,
    /// and current state. The description is extracted from the JSON descriptor's
    /// `"description"` field when present.
    pub async fn list_all(&self) -> Vec<ToolSummary> {
        self.inner
            .read()
            .await
            .values()
            .map(|rec| {
                let name = rec.entry.name.as_str().to_string();
                let namespace = name.split('/').next().unwrap_or("").to_string();
                let description = rec
                    .entry
                    .descriptor
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let state = match rec.entry.state {
                    crate::types::tool::ToolState::Available => "available",
                    crate::types::tool::ToolState::Unavailable => "unavailable",
                    crate::types::tool::ToolState::Degraded => "degraded",
                }
                .to_string();
                ToolSummary {
                    name,
                    namespace,
                    description,
                    state,
                }
            })
            .collect()
    }
}

/// A lightweight summary of a registered tool, returned by `list_all()`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolSummary {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub state: String,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool::ToolName;

    fn make_entry(name: &str) -> ToolEntry {
        ToolEntry::new(
            ToolName::parse(name).unwrap(),
            "test-svc".into(),
            ToolState::Available,
            ToolVisibility::All,
            serde_json::json!({}),
        )
    }

    #[tokio::test]
    async fn tool_changed_event_fires_on_add() {
        let (reg, mut events) = ToolRegistry::new_with_events();
        reg.add("svc", vec![make_entry("ns/tool")]).await.unwrap();
        let evt = events.recv().await.unwrap();
        assert_eq!(evt.op, "added");
        assert!(evt.tools.contains(&"ns/tool".to_string()));
    }

    #[tokio::test]
    async fn tool_changed_event_fires_on_remove() {
        let (reg, mut events) = ToolRegistry::new_with_events();
        reg.add("svc", vec![make_entry("ns/tool")]).await.unwrap();
        let _ = events.recv().await; // consume the add event
        reg.remove("svc", &["ns/tool"], "test", false)
            .await
            .unwrap();
        let evt = events.recv().await.unwrap();
        assert!(evt.op.contains("removed"));
        assert!(evt.tools.contains(&"ns/tool".to_string()));
    }

    #[tokio::test]
    async fn remove_with_drain_marks_unavailable_then_removes() {
        let (reg, _events) = ToolRegistry::new_with_events();
        reg.add("svc", vec![make_entry("x/drain")]).await.unwrap();

        // With no in-flight calls, drain should complete immediately.
        reg.remove("svc", &["x/drain"], "drain-test", true)
            .await
            .unwrap();

        assert!(reg.lookup("x/drain").await.is_err());
    }

    #[tokio::test]
    async fn remove_nonexistent_tool_emits_no_event() {
        let (reg, mut events) = ToolRegistry::new_with_events();
        reg.add("svc", vec![make_entry("a/b")]).await.unwrap();
        let _ = events.recv().await; // consume add event

        // Remove a name that doesn't exist — filter ensures no event.
        reg.remove("svc", &["no/such"], "test", false)
            .await
            .unwrap();

        // No event should arrive; channel should be empty.
        // Use a short timeout to confirm.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(20), events.recv()).await;
        assert!(result.is_err(), "expected timeout, got an event");
    }

    #[tokio::test]
    async fn atp_bridge_forwards_tool_events_to_event_bus() {
        use crate::gateway::event_bus::AtpEventBus;

        let reg = Arc::new(ToolRegistry::new());
        let bus = Arc::new(AtpEventBus::new());

        reg.clone().start_atp_bridge(Arc::clone(&bus)).await;

        reg.add("test-svc", vec![make_entry("test/echo")])
            .await
            .unwrap();

        let mut rx = bus.subscribe();
        let ev = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("event should be received")
            .expect("event should be ok");

        assert_eq!(
            ev.event.event,
            crate::gateway::atp::types::AtpEventKind::ToolChanged
        );
    }
}
