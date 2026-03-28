use super::entry::ToolEntry;
use super::events::ToolChangedEvent;
use crate::error::AvixError;
use crate::types::tool::{ToolState, ToolVisibility};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, Semaphore};

const EVENT_CAPACITY: usize = 64;

pub struct ToolCallGuard {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

struct ToolRecord {
    entry: ToolEntry,
    semaphore: Arc<Semaphore>,
}

pub struct ToolRegistry {
    inner: Arc<RwLock<HashMap<String, ToolRecord>>>,
    events: broadcast::Sender<ToolChangedEvent>,
}

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
        ToolEntry {
            name: ToolName::parse(name).unwrap(),
            owner: "test-svc".into(),
            state: ToolState::Available,
            visibility: ToolVisibility::All,
            descriptor: serde_json::json!({}),
        }
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
}
