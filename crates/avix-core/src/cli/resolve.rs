use crate::bootstrap::phase1;
use crate::config::users::UsersConfig;
use crate::error::AvixError;
use crate::memfs::{MemFs, VfsPath};
use crate::params::limits::{AgentLimits, LimitsFile, LimitsLayer};
use crate::params::resolved_file::ResolvedFile;
use crate::params::resolver::{ParamResolver, ResolverInput, ResolverInputLoader};
use std::path::PathBuf;

pub struct ResolveParams {
    pub root: PathBuf,
    pub kind: String,
    pub username: String,
    pub agent_name: Option<String>,
    pub explain: bool,
    pub limits_only: bool,
    pub extra_crew: Option<String>,
    pub dry_run: bool,
}

pub struct ResolveResult {
    pub output: String,
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Resolve agent parameters for the given user and print a `kind: Resolved` YAML document.
///
/// This is a pure I/O function: it opens the VFS, loads defaults and limits from disk
/// and compiled-in sources, runs the resolution engine, and returns the serialised output.
pub async fn run_resolve(params: ResolveParams) -> Result<ResolveResult, AvixError> {
    // 1. Load user record from {root}/etc/users.yaml to get crew memberships
    let users_path = params.root.join("etc/users.yaml");
    let users_yaml = std::fs::read_to_string(&users_path)
        .map_err(|e| AvixError::ConfigParse(format!("cannot read users.yaml: {e}")))?;
    let users_config = UsersConfig::from_str(&users_yaml)?;

    let user = users_config.find_user(&params.username).ok_or_else(|| {
        AvixError::ConfigParse(format!(
            "user '{}' not found in users.yaml",
            params.username
        ))
    })?;

    let mut crews = user.crews.clone();
    if let Some(extra) = &params.extra_crew {
        if !crews.contains(extra) {
            crews.push(extra.clone());
        }
    }

    // 2. Build in-memory VFS with compiled-in system defaults/limits (via phase1)
    let vfs = MemFs::new();
    phase1::run(&vfs).await;

    // 3. Load per-crew and per-user defaults/limits from disk into VFS
    for crew in &crews {
        populate_vfs_from_disk(
            &vfs,
            &params.root.join(format!("data/crews/{crew}/defaults.yaml")),
            &format!("/crews/{crew}/defaults.yaml"),
        )
        .await;
        populate_vfs_from_disk(
            &vfs,
            &params.root.join(format!("data/crews/{crew}/limits.yaml")),
            &format!("/crews/{crew}/limits.yaml"),
        )
        .await;
    }
    populate_vfs_from_disk(
        &vfs,
        &params
            .root
            .join(format!("data/users/{}/defaults.yaml", params.username)),
        &format!("/users/{}/defaults.yaml", params.username),
    )
    .await;
    populate_vfs_from_disk(
        &vfs,
        &params
            .root
            .join(format!("data/users/{}/limits.yaml", params.username)),
        &format!("/users/{}/limits.yaml", params.username),
    )
    .await;

    // 4. Build resolver input via the VFS loader
    let loader = ResolverInputLoader::new(&vfs);
    let input = loader.load(&params.username, &crews).await?;

    // 5. --limits-only: compute and return effective limits without running full resolution
    if params.limits_only {
        let effective = compute_effective_limits(&input);
        let yaml = LimitsFile::from_agent_limits(LimitsLayer::System, None, &effective)?;
        return Ok(ResolveResult { output: yaml });
    }

    // 6. Run full resolution
    let (resolved, annotations) =
        ParamResolver::resolve(&input).map_err(|e| AvixError::ConfigParse(e.to_string()))?;

    let annotations = if params.explain {
        Some(annotations)
    } else {
        None
    };

    let file = ResolvedFile::new(&params.username, None, crews, resolved, vec![], annotations);
    let output = file.to_yaml()?;

    Ok(ResolveResult { output })
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Silently read a disk file and write it into the VFS. Missing files are ignored.
async fn populate_vfs_from_disk(vfs: &MemFs, disk_path: &std::path::Path, vfs_path: &str) {
    if let Ok(bytes) = std::fs::read(disk_path) {
        if let Ok(path) = VfsPath::parse(vfs_path) {
            let _ = vfs.write(&path, bytes).await;
        }
    }
}

/// Compute the effective (tightest) limits by intersecting system, crew, and user limits.
fn compute_effective_limits(input: &ResolverInput) -> AgentLimits {
    let mut effective = input.system_limits.clone();
    for ll in &input.crew_limits {
        effective = effective.intersect(&ll.limits);
    }
    if let Some(ul) = &input.user_limits {
        effective = effective.intersect(&ul.limits);
    }
    effective
}
