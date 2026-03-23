use crate::bootstrap::phase1;
use crate::config::users::UsersConfig;
use crate::error::AvixError;
use crate::memfs::{LocalProvider, VfsRouter};
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
/// Builds a `VfsRouter` with `LocalProvider` mounts for the relevant crew and user
/// directories so that the resolver can read defaults/limits directly from disk.
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

    // 2. Build VfsRouter with compiled-in system defaults/limits (via phase1)
    let vfs = VfsRouter::new();
    phase1::run(&vfs).await;

    // 3. Mount per-crew directories so the resolver can read defaults/limits from disk
    for crew in &crews {
        let crew_dir = params.root.join(format!("data/crews/{crew}"));
        if crew_dir.exists() {
            if let Ok(provider) = LocalProvider::new(&crew_dir) {
                vfs.mount(format!("/crews/{crew}"), provider).await;
            }
        }
    }

    // 4. Mount per-user directory
    let user_dir = params.root.join(format!("data/users/{}", params.username));
    if user_dir.exists() {
        if let Ok(provider) = LocalProvider::new(&user_dir) {
            vfs.mount(format!("/users/{}", params.username), provider)
                .await;
        }
    }

    // 5. Build resolver input via the VFS loader
    let loader = ResolverInputLoader::new(&vfs);
    let input = loader.load(&params.username, &crews).await?;

    // 6. --limits-only: compute and return effective limits without running full resolution
    if params.limits_only {
        let effective = compute_effective_limits(&input);
        let yaml = LimitsFile::from_agent_limits(LimitsLayer::System, None, &effective)?;
        return Ok(ResolveResult { output: yaml });
    }

    // 7. Run full resolution
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
