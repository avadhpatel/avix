use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;

use crate::config::MemoryConfig;
use crate::error::AvixError;
use crate::memfs::VfsRouter;

use super::store;
use super::tools;
use super::vfs_layout::{memory_svc_status_path, MemorySvcStatus};

use tracing::instrument;

#[derive(Debug)]
pub struct MemoryService {
    pub(super) vfs: Arc<VfsRouter>,
    pub(super) kernel_config: Arc<MemoryConfig>,
}

impl MemoryService {
    #[instrument(skip_all)]
    pub fn new(vfs: Arc<VfsRouter>, kernel_config: Arc<MemoryConfig>) -> Self {
        Self { vfs, kernel_config }
    }

    /// Called at service startup — writes `/proc/services/memory/status.yaml`.
    #[instrument(skip(self))]
    pub async fn start(&self) -> Result<(), AvixError> {
        let status = MemorySvcStatus {
            healthy: true,
            total_episodic_records: 0,
            total_semantic_records: 0,
            active_session_grants: 0,
            updated_at: Utc::now(),
        };
        let yaml =
            serde_yaml::to_string(&status).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        use crate::memfs::VfsPath;
        let path = VfsPath::parse(memory_svc_status_path())
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        self.vfs.write(&path, yaml.into_bytes()).await
    }

    /// Dispatch a tool call to the correct handler.
    #[instrument(skip(self, params))]
    pub async fn dispatch(
        &self,
        tool_name: &str,
        params: Value,
        caller: &CallerContext,
    ) -> Result<Value, AvixError> {
        match tool_name {
            "memory/retrieve" => tools::retrieve::handle(self, params, caller).await,
            "memory/log-event" => tools::log_event::handle(self, params, caller).await,
            "memory/store-fact" => tools::store_fact::handle(self, params, caller).await,
            "memory/get-fact" => tools::get_fact::handle(self, params, caller).await,
            "memory/update-preference" => {
                tools::update_preference::handle(self, params, caller).await
            }
            "memory/get-preferences" => tools::get_preferences::handle(self, params, caller).await,
            "memory/forget" => tools::forget::handle(self, params, caller).await,
            "memory/share-request" => tools::share_request::handle(self, params, caller).await,
            _ => Err(AvixError::NotFound(format!(
                "unknown memory tool: {tool_name}"
            ))),
        }
    }
}

// ── CallerContext ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CallerContext {
    pub pid: u64,
    pub agent_name: String,
    pub owner: String,
    pub session_id: String,
    pub granted_tools: Vec<String>,
}

impl CallerContext {
    #[instrument(skip(self))]
    pub fn has_capability(&self, cap: &str) -> bool {
        match cap {
            "memory:read" => self.granted_tools.iter().any(|t| {
                t == "memory/retrieve" || t == "memory/get-fact" || t == "memory/get-preferences"
            }),
            "memory:write" => self.granted_tools.iter().any(|t| {
                t == "memory/log-event"
                    || t == "memory/store-fact"
                    || t == "memory/update-preference"
            }),
            "memory:share" => self
                .granted_tools
                .contains(&"memory/share-request".to_string()),
            _ => false,
        }
    }
}

// Make vfs and kernel_config accessible to tool handlers via the service ref
impl MemoryService {
    #[instrument(skip(self))]
    pub(super) fn vfs(&self) -> &VfsRouter {
        &self.vfs
    }

    #[instrument(skip(self))]
    pub(super) fn default_retrieve_limit(&self) -> usize {
        self.kernel_config.retrieval.default_limit as usize
    }
}

// ── find_record_by_id ─────────────────────────────────────────────────────────

/// Linear scan over episodic + semantic dirs to find a record with the given ID.
/// Returns the VFS path of the matching record, or None.
///
/// TODO memory-gap-E: use index for O(1) lookup
#[instrument(skip(vfs))]
pub(super) async fn find_record_by_id(
    vfs: &VfsRouter,
    owner: &str,
    agent_name: &str,
    id: &str,
) -> Option<String> {
    let dirs = [
        format!("/users/{}/memory/{}/episodic", owner, agent_name),
        format!("/users/{}/memory/{}/semantic", owner, agent_name),
    ];
    for dir in &dirs {
        if let Ok(records) = store::list_records(vfs, dir).await {
            for record in records {
                if record.metadata.id == id {
                    // Reconstruct path from metadata
                    let path = match record.metadata.record_type {
                        crate::memory_svc::MemoryRecordType::Episodic => {
                            crate::memory_svc::MemoryRecord::vfs_path_episodic(
                                owner,
                                agent_name,
                                &record.metadata.created_at,
                                id,
                            )
                        }
                        crate::memory_svc::MemoryRecordType::Semantic => {
                            if let Some(ref key) = record.spec.key {
                                crate::memory_svc::MemoryRecord::vfs_path_semantic(
                                    owner, agent_name, key,
                                )
                            } else {
                                continue;
                            }
                        }
                    };
                    return Some(path);
                }
            }
        }
    }
    None
}
