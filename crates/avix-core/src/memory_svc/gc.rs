use chrono::Utc;

use crate::error::AvixError;
use crate::memfs::{VfsPath, VfsRouter};

use super::schema::MemoryGrant;
use super::store;

// ── GcReport ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct GcReport {
    pub records_deleted: u64,
    pub bytes_freed: u64,
}

// ── GC — episodic record expiry ───────────────────────────────────────────────

/// Delete episodic records older than `retention_days` for the given `(owner, agent_name)` pairs.
///
/// Skips pinned records. Idempotent.
///
/// Note: the caller (kernel cron job) is responsible for supplying the list of active
/// `(owner, agent_name)` pairs from the user registry or process table.
pub async fn gc_episodic_records(
    vfs: &VfsRouter,
    agents: &[(&str, &str)],
    retention_days: u32,
) -> Result<GcReport, AvixError> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut report = GcReport::default();

    for (owner, agent_name) in agents {
        let episodic_dir = format!("/users/{}/memory/{}/episodic", owner, agent_name);
        let episodic_path = match VfsPath::parse(&episodic_dir) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let entries = vfs.list(&episodic_path).await.unwrap_or_default();

        for filename in entries.iter().filter(|e| e.ends_with(".yaml") && *e != ".keep") {
            let full = format!("{}/{}", episodic_dir, filename);
            let record = match store::read_record(vfs, &full).await {
                Ok(r) => r,
                Err(_) => continue,
            };

            if record.metadata.pinned {
                continue;
            }

            if record.metadata.created_at < cutoff {
                let vfs_path = match VfsPath::parse(&full) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if let Ok(bytes) = vfs.read(&vfs_path).await {
                    report.bytes_freed += bytes.len() as u64;
                }
                vfs.delete(&vfs_path).await.ok();
                report.records_deleted += 1;
            }
        }
    }

    Ok(report)
}

// ── GC — expired session grants ───────────────────────────────────────────────

/// Prune `MemoryGrant` records from `/proc/services/memory/agents/<agent>/grants/`
/// that have a non-null `spec.expiresAt` in the past.
///
/// `agent_names` — the caller (kernel cron job) supplies known active agent names.
/// Returns the number of grants pruned.
pub async fn prune_expired_grants(
    vfs: &VfsRouter,
    agent_names: &[&str],
) -> Result<u32, AvixError> {
    let grant_root = "/proc/services/memory/agents";
    let mut pruned = 0u32;
    let now = Utc::now();

    for agent_name in agent_names {
        let grants_dir = format!("{}/{}/grants", grant_root, agent_name);
        let grants_path = match VfsPath::parse(&grants_dir) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let entries = vfs.list(&grants_path).await.unwrap_or_default();

        for filename in entries.iter().filter(|e| e.ends_with(".yaml")) {
            let full = format!("{}/{}", grants_dir, filename);
            let vfs_path = match VfsPath::parse(&full) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let bytes = match vfs.read(&vfs_path).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let yaml = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };

            if let Ok(grant) = MemoryGrant::from_yaml(&yaml) {
                let expired = grant
                    .spec
                    .expires_at
                    .map(|exp| exp < now)
                    .unwrap_or(false);

                if expired {
                    vfs.delete(&vfs_path).await.ok();
                    pruned += 1;
                }
            }
        }
    }

    Ok(pruned)
}
