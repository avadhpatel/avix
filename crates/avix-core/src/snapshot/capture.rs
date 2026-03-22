use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub id: String,
    pub agent_name: String,
    pub agent_pid: u32,
    pub spawned_by: u32,
    pub goal: String,
    pub created_at: DateTime<Utc>,
    pub granted_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub meta: SnapshotMeta,
    pub message_history: Vec<SnapshotMessage>,
}

impl Snapshot {
    pub fn new(
        agent_name: String,
        agent_pid: u32,
        spawned_by: u32,
        goal: String,
        granted_tools: Vec<String>,
        message_history: Vec<SnapshotMessage>,
    ) -> Self {
        Self {
            meta: SnapshotMeta {
                id: Uuid::new_v4().to_string(),
                agent_name,
                agent_pid,
                spawned_by,
                goal,
                created_at: Utc::now(),
                granted_tools,
            },
            message_history,
        }
    }

    pub fn message_count(&self) -> usize {
        self.message_history.len()
    }
}
