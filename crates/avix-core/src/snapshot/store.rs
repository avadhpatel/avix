use std::collections::HashMap;

use thiserror::Error;
use tokio::sync::RwLock;

use super::capture::SnapshotFile;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot not found: {0}")]
    NotFound(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

pub struct SnapshotStore {
    // agent_name → Vec<SnapshotFile>
    store: RwLock<HashMap<String, Vec<SnapshotFile>>>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }

    /// Save a snapshot; returns the snapshot name (the stable ID).
    pub async fn save(&self, snap: SnapshotFile) -> Result<String, SnapshotError> {
        let name = snap.metadata.name.clone();
        let agent = snap.metadata.agent_name.clone();
        self.store
            .write()
            .await
            .entry(agent)
            .or_default()
            .push(snap);
        Ok(name)
    }

    /// Load a snapshot by name.
    pub async fn load(&self, name: &str) -> Result<SnapshotFile, SnapshotError> {
        let store = self.store.read().await;
        for snaps in store.values() {
            if let Some(snap) = snaps.iter().find(|s| s.metadata.name == name) {
                return Ok(snap.clone());
            }
        }
        Err(SnapshotError::NotFound(name.to_string()))
    }

    /// List all snapshots for an agent.
    pub async fn list(&self, agent_name: &str) -> Vec<SnapshotFile> {
        self.store
            .read()
            .await
            .get(agent_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Delete a snapshot by name.
    pub async fn delete(&self, name: &str) -> Result<(), SnapshotError> {
        let mut store = self.store.write().await;
        for snaps in store.values_mut() {
            if let Some(pos) = snaps.iter().position(|s| s.metadata.name == name) {
                snaps.remove(pos);
                return Ok(());
            }
        }
        Err(SnapshotError::NotFound(name.to_string()))
    }

    pub async fn snapshot_count(&self, agent_name: &str) -> usize {
        self.store
            .read()
            .await
            .get(agent_name)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::capture::{
        CapturedBy, SnapshotEnvironment, SnapshotFile, SnapshotMemory, SnapshotMetadata,
        SnapshotSpec, SnapshotTrigger,
    };

    fn make_snap(agent_name: &str, source_pid: u32) -> SnapshotFile {
        let captured_at = chrono::Utc::now();
        let name = SnapshotFile::make_name(agent_name, &captured_at);
        SnapshotFile::new(
            SnapshotMetadata {
                name,
                agent_name: agent_name.to_string(),
                source_pid,
                captured_at,
                captured_by: CapturedBy::Kernel,
                trigger: SnapshotTrigger::Manual,
            },
            SnapshotSpec {
                goal: "do stuff".into(),
                context_summary: "working".into(),
                context_token_count: 100,
                memory: SnapshotMemory::default(),
                pending_requests: vec![],
                pipes: vec![],
                environment: SnapshotEnvironment {
                    temperature: 0.7,
                    capability_token: "sha256:abc".into(),
                    granted_tools: vec!["fs/read".into()],
                },
                checksum: "sha256:placeholder".into(),
            },
        )
    }

    // T-SA-05: SnapshotStore async save + load + list
    #[tokio::test]
    async fn snapshot_store_save_load_list() {
        let store = SnapshotStore::new();
        let snap = make_snap("researcher", 42);
        let name = store.save(snap.clone()).await.unwrap();
        let loaded = store.load(&name).await.unwrap();
        assert_eq!(loaded.metadata.source_pid, 42);
        let list = store.list("researcher").await;
        assert_eq!(list.len(), 1);
    }

    // T-SA-06: SnapshotStore delete
    #[tokio::test]
    async fn snapshot_store_delete() {
        let store = SnapshotStore::new();
        let name = store.save(make_snap("researcher", 42)).await.unwrap();
        store.delete(&name).await.unwrap();
        assert_eq!(store.snapshot_count("researcher").await, 0);
    }

    #[tokio::test]
    async fn snapshot_store_load_not_found() {
        let store = SnapshotStore::new();
        assert!(matches!(
            store.load("nonexistent").await,
            Err(SnapshotError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn snapshot_store_delete_not_found() {
        let store = SnapshotStore::new();
        assert!(matches!(
            store.delete("nonexistent").await,
            Err(SnapshotError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn snapshot_store_multiple_agents() {
        let store = SnapshotStore::new();
        store.save(make_snap("agent-a", 1)).await.unwrap();
        store.save(make_snap("agent-b", 2)).await.unwrap();
        assert_eq!(store.list("agent-a").await.len(), 1);
        assert_eq!(store.list("agent-b").await.len(), 1);
    }
}
