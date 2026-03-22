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
        _reason: &str,
        drain: bool,
    ) -> Result<(), AvixError> {
        if drain {
            // Wait for in-flight calls to complete
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

        let mut guard = self.inner.write().await;
        let removed: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        for name in &removed {
            guard.remove(name.as_str());
        }
        let _ = self.events.send(ToolChangedEvent {
            op: "removed".into(),
            tools: removed,
        });
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
