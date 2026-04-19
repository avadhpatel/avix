use chrono::Utc;

use crate::error::AvixError;
use crate::memfs::VfsPath;

use super::schema::{
    new_memory_id, MemoryGrant, MemoryGrantGrantee, MemoryGrantGrantor, MemoryGrantMetadata,
    MemoryGrantScope, MemoryGrantSpec,
};
use super::service::MemoryService;
use super::vfs_layout::memory_agent_grants_path;

use tracing::instrument;

// ── HIL approval handler ──────────────────────────────────────────────────────

/// Called by the kernel's approval handler after `ApprovalToken` is atomically consumed.
///
/// Creates a `MemoryGrant` record:
/// - **Session scope** → `/proc/services/memory/agents/<target>/grants/<id>.yaml`
/// - **Permanent scope** → `/users/<owner>/memory/<grantor-agent>/grants/<id>.yaml`
#[allow(clippy::too_many_arguments)]
#[instrument]
pub async fn on_memory_share_approved(
    svc: &MemoryService,
    hil_id: &str,
    _caller_pid: u32,
    target_agent: &str,
    record_ids: Vec<String>,
    scope: MemoryGrantScope,
    owner: &str,
    session_id: &str,
    approving_user: &str,
    grantor_agent: &str,
) -> Result<(), AvixError> {
    let grant_id = format!("grant-{}", new_memory_id());

    let grant = MemoryGrant::new(
        MemoryGrantMetadata {
            id: grant_id.clone(),
            granted_at: Utc::now(),
            granted_by: approving_user.to_string(),
            hil_id: hil_id.to_string(),
        },
        MemoryGrantSpec {
            grantor: MemoryGrantGrantor {
                agent_name: grantor_agent.to_string(),
                owner: owner.to_string(),
            },
            grantee: MemoryGrantGrantee {
                agent_name: target_agent.to_string(),
                owner: owner.to_string(),
            },
            records: record_ids,
            scope: scope.clone(),
            session_id: session_id.to_string(),
            expires_at: None,
        },
    );

    let yaml = grant
        .to_yaml()
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    let path = match scope {
        MemoryGrantScope::Session => memory_agent_grants_path(target_agent, &grant_id),
        MemoryGrantScope::Permanent => MemoryGrant::vfs_path(owner, grantor_agent, &grant_id),
    };

    let vfs_path = VfsPath::parse(&path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    svc.vfs.write(&vfs_path, yaml.into_bytes()).await
}

// ── Session grant cleanup ─────────────────────────────────────────────────────

/// Delete all session-scoped grants for an agent when its session closes.
///
/// Scans `/proc/services/memory/agents/<agent>/grants/` and removes entries
/// where `spec.scope == session` and `spec.sessionId == session_id`.
#[instrument]
pub async fn cleanup_session_grants(
    svc: &MemoryService,
    agent_name: &str,
    session_id: &str,
) -> Result<(), AvixError> {
    let grant_dir = format!("/proc/services/memory/agents/{}/grants", agent_name);
    let vfs_dir = VfsPath::parse(&grant_dir).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    let entries = svc.vfs.list(&vfs_dir).await.unwrap_or_default();

    for filename in entries.iter().filter(|e| e.ends_with(".yaml")) {
        let full_path = format!("{}/{}", grant_dir, filename);
        let vfs_path = match VfsPath::parse(&full_path) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let bytes = match svc.vfs.read(&vfs_path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let yaml = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        if let Ok(grant) = MemoryGrant::from_yaml(&yaml) {
            if grant.spec.scope == MemoryGrantScope::Session && grant.spec.session_id == session_id
            {
                svc.vfs.delete(&vfs_path).await.ok();
            }
        }
    }

    Ok(())
}

// ── Grant loader helper ───────────────────────────────────────────────────────

/// Read and parse a `MemoryGrant` from a VFS path.
#[instrument]
pub async fn load_grant(svc: &MemoryService, path: &str) -> Result<MemoryGrant, AvixError> {
    let vfs_path = VfsPath::parse(path).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let bytes = svc
        .vfs
        .read(&vfs_path)
        .await
        .map_err(|_| AvixError::NotFound(format!("grant not found: {path}")))?;
    let yaml = String::from_utf8(bytes).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    MemoryGrant::from_yaml(&yaml)
}
