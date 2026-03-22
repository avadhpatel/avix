use std::collections::HashMap;
use std::sync::RwLock;

use thiserror::Error;

use super::capture::Snapshot;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot not found: {0}")]
    NotFound(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

pub struct SnapshotStore {
    // agent_name -> Vec<Snapshot>
    store: RwLock<HashMap<String, Vec<Snapshot>>>,
}

impl SnapshotStore {
    pub fn new() -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
        }
    }

    pub fn save(&self, snap: Snapshot) -> Result<String, SnapshotError> {
        let id = snap.meta.id.clone();
        let agent = snap.meta.agent_name.clone();
        let mut store = self.store.write().unwrap();
        store.entry(agent).or_default().push(snap);
        Ok(id)
    }

    pub fn load(&self, id: &str) -> Result<Snapshot, SnapshotError> {
        let store = self.store.read().unwrap();
        for snaps in store.values() {
            if let Some(snap) = snaps.iter().find(|s| s.meta.id == id) {
                return Ok(snap.clone());
            }
        }
        Err(SnapshotError::NotFound(id.to_string()))
    }

    pub fn list(&self, agent_name: &str) -> Vec<Snapshot> {
        let store = self.store.read().unwrap();
        store.get(agent_name).cloned().unwrap_or_default()
    }

    pub fn delete(&self, id: &str) -> Result<(), SnapshotError> {
        let mut store = self.store.write().unwrap();
        for snaps in store.values_mut() {
            if let Some(pos) = snaps.iter().position(|s| s.meta.id == id) {
                snaps.remove(pos);
                return Ok(());
            }
        }
        Err(SnapshotError::NotFound(id.to_string()))
    }

    pub fn snapshot_count(&self, agent_name: &str) -> usize {
        self.store
            .read()
            .unwrap()
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
    use crate::snapshot::capture::{Snapshot, SnapshotMessage};

    fn make_snap(name: &str) -> Snapshot {
        Snapshot::new(
            name.into(),
            42,
            1,
            "do stuff".into(),
            vec!["fs/read".into()],
            vec![SnapshotMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
        )
    }

    #[test]
    fn test_save_and_load() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap.clone()).unwrap();
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.meta.agent_name, "agent-x");
    }

    #[test]
    fn test_message_history_preserved() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap).unwrap();
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.message_history[0].content, "hello");
        assert_eq!(loaded.message_count(), 1);
    }

    #[test]
    fn test_granted_tools_preserved() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap).unwrap();
        let loaded = store.load(&id).unwrap();
        assert!(loaded.meta.granted_tools.contains(&"fs/read".to_string()));
    }

    #[test]
    fn test_list_snapshots() {
        let store = SnapshotStore::new();
        store.save(make_snap("agent-x")).unwrap();
        store.save(make_snap("agent-x")).unwrap();
        assert_eq!(store.list("agent-x").len(), 2);
        assert_eq!(store.list("other").len(), 0);
    }

    #[test]
    fn test_delete_snapshot() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap).unwrap();
        store.delete(&id).unwrap();
        assert!(matches!(store.load(&id), Err(SnapshotError::NotFound(_))));
    }

    #[test]
    fn test_load_not_found() {
        let store = SnapshotStore::new();
        assert!(matches!(
            store.load("nonexistent"),
            Err(SnapshotError::NotFound(_))
        ));
    }

    #[test]
    fn test_snapshot_unique_ids() {
        let snap1 = make_snap("agent-x");
        let snap2 = make_snap("agent-x");
        assert_ne!(snap1.meta.id, snap2.meta.id);
    }

    #[test]
    fn test_snapshot_yaml_roundtrip() {
        let snap = make_snap("agent-x");
        let yaml = serde_yaml::to_string(&snap).unwrap();
        let restored: Snapshot = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(restored.meta.agent_name, "agent-x");
        assert_eq!(restored.message_history[0].content, "hello");
    }

    #[test]
    fn test_goal_preserved() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap).unwrap();
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.meta.goal, "do stuff");
    }

    #[test]
    fn test_spawned_by_preserved() {
        let store = SnapshotStore::new();
        let snap = make_snap("agent-x");
        let id = store.save(snap).unwrap();
        let loaded = store.load(&id).unwrap();
        assert_eq!(loaded.meta.spawned_by, 1);
    }

    #[test]
    fn test_snapshot_count() {
        let store = SnapshotStore::new();
        assert_eq!(store.snapshot_count("agent-x"), 0);
        store.save(make_snap("agent-x")).unwrap();
        assert_eq!(store.snapshot_count("agent-x"), 1);
    }

    #[test]
    fn test_multiple_agents_independent() {
        let store = SnapshotStore::new();
        store.save(make_snap("agent-a")).unwrap();
        store.save(make_snap("agent-b")).unwrap();
        assert_eq!(store.list("agent-a").len(), 1);
        assert_eq!(store.list("agent-b").len(), 1);
    }

    #[test]
    fn test_delete_nonexistent() {
        let store = SnapshotStore::new();
        let res = store.delete("nonexistent-id");
        assert!(matches!(res, Err(SnapshotError::NotFound(_))));
    }

    #[test]
    fn test_created_at_set() {
        let snap = make_snap("agent-x");
        let now = chrono::Utc::now();
        // created_at should be close to now
        let diff = now - snap.meta.created_at;
        assert!(diff.num_seconds().abs() < 5);
    }
}
