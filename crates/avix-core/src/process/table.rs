use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use chrono::{DateTime, Utc};

use super::entry::{ProcessEntry, ProcessKind, ProcessStatus};
use crate::error::AvixError;
use crate::types::Pid;

#[derive(Debug, Default)]
pub struct ProcessTable {
    inner: Arc<RwLock<HashMap<u32, ProcessEntry>>>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, entry: ProcessEntry) {
        self.inner.write().await.insert(entry.pid.as_u32(), entry);
    }

    pub async fn remove(&self, pid: Pid) {
        self.inner.write().await.remove(&pid.as_u32());
    }

    pub async fn get(&self, pid: Pid) -> Option<ProcessEntry> {
        self.inner.read().await.get(&pid.as_u32()).cloned()
    }

    pub async fn set_status(&self, pid: Pid, status: ProcessStatus) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        match guard.get_mut(&pid.as_u32()) {
            Some(e) => {
                e.status = status;
                Ok(())
            }
            None => Err(AvixError::InvalidPid(pid.to_string())),
        }
    }

    pub async fn list_all(&self) -> Vec<ProcessEntry> {
        self.inner.read().await.values().cloned().collect()
    }

    pub async fn list_by_kind(&self, kind: ProcessKind) -> Vec<ProcessEntry> {
        self.inner
            .read()
            .await
            .values()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }

    pub async fn list_by_status(&self, status: ProcessStatus) -> Vec<ProcessEntry> {
        self.inner
            .read()
            .await
            .values()
            .filter(|e| e.status == status)
            .cloned()
            .collect()
    }

    pub async fn list_children(&self, parent: Pid) -> Vec<ProcessEntry> {
        self.inner
            .read()
            .await
            .values()
            .filter(|e| e.parent == Some(parent))
            .cloned()
            .collect()
    }

    pub async fn find_by_name(&self, name: &str) -> Option<ProcessEntry> {
        self.inner
            .read()
            .await
            .values()
            .find(|e| e.name == name)
            .cloned()
    }

    pub async fn count(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Update the capability fields for an agent: the granted tool names and optional expiry.
    /// Called by `RuntimeExecutor` at spawn and on token renewal.
    pub async fn set_token(
        &self,
        pid: Pid,
        granted_tools: Vec<String>,
        token_expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        match guard.get_mut(&pid.as_u32()) {
            Some(e) => {
                e.granted_tools = granted_tools;
                e.token_expires_at = token_expires_at;
                Ok(())
            }
            None => Err(AvixError::InvalidPid(pid.to_string())),
        }
    }

    /// Increment the tool-chain depth counter for the current turn.
    /// Called each time a tool call is dispatched within a turn.
    pub async fn increment_chain_depth(&self, pid: Pid) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        match guard.get_mut(&pid.as_u32()) {
            Some(e) => {
                e.tool_chain_depth = e.tool_chain_depth.saturating_add(1);
                Ok(())
            }
            None => Err(AvixError::InvalidPid(pid.to_string())),
        }
    }

    /// Reset the tool-chain depth to 0 at the start of each new turn.
    pub async fn reset_chain_depth(&self, pid: Pid) -> Result<(), AvixError> {
        let mut guard = self.inner.write().await;
        match guard.get_mut(&pid.as_u32()) {
            Some(e) => {
                e.tool_chain_depth = 0;
                Ok(())
            }
            None => Err(AvixError::InvalidPid(pid.to_string())),
        }
    }
}
