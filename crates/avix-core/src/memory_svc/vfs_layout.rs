use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

// ── VFS path helpers ──────────────────────────────────────────────────────────

pub fn memory_svc_status_path() -> &'static str {
    "/proc/services/memory/status.yaml"
}

pub fn memory_agent_stats_path(agent_name: &str) -> String {
    format!("/proc/services/memory/agents/{}/stats.yaml", agent_name)
}

pub fn memory_agent_grants_path(agent_name: &str, grant_id: &str) -> String {
    format!(
        "/proc/services/memory/agents/{}/grants/{}.yaml",
        agent_name, grant_id
    )
}

// ── Status structs ────────────────────────────────────────────────────────────

/// Runtime status of `memory.svc`, written to `/proc/services/memory/status.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySvcStatus {
    pub healthy: bool,
    pub total_episodic_records: u64,
    pub total_semantic_records: u64,
    pub active_session_grants: u32,
    pub updated_at: DateTime<Utc>,
}

/// Per-agent memory statistics, written to `/proc/services/memory/agents/<name>/stats.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAgentStats {
    pub agent_name: String,
    pub episodic_record_count: u32,
    pub semantic_record_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_write_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_retrieval_at: Option<DateTime<Utc>>,
}

// ── Directory bootstrap ───────────────────────────────────────────────────────

/// Initialise all memory directories for an agent within a user's memory tree.
///
/// Called from `RuntimeExecutor` after VFS setup, before `SIGSTART`. Idempotent.
pub async fn init_user_memory_tree(
    vfs: &VfsRouter,
    owner: &str,
    agent_name: &str,
) -> Result<(), AvixError> {
    let base = format!("/users/{}/memory/{}", owner, agent_name);

    // Primary subdirectories
    for subdir in &["episodic", "semantic", "preferences", "grants"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await?;
    }

    // Index subdirectories (built by memory.svc, not by agents)
    for subdir in &["episodic/index", "semantic/index"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await?;
    }

    Ok(())
}

/// Initialise the shared memory tree for a crew.
///
/// Called from the crew creation path. Idempotent.
pub async fn init_crew_memory_tree(vfs: &VfsRouter, crew_name: &str) -> Result<(), AvixError> {
    let base = format!("/crews/{}/memory/shared", crew_name);

    for subdir in &["episodic", "semantic", "episodic/index", "semantic/index"] {
        let path = VfsPath::parse(&format!("{}/{}", base, subdir))
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        vfs.ensure_dir(&path).await?;
    }

    Ok(())
}
