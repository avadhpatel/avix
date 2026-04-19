use crate::memfs::{VfsPath, VfsRouter};
use tracing::instrument;

use crate::params::defaults::{system_agent_defaults, DefaultsFile, DefaultsLayer};
use crate::params::limits::{system_agent_limits, LimitsFile, LimitsLayer};

/// Phase 1: Write the kernel VFS skeleton.
///
/// Creates directory anchors and compiles-in default/limit files so that
/// agents spawned later can read system defaults from `/kernel/defaults/`.
/// All paths written here are kernel-owned ephemeral trees — they are
/// re-created on every boot, never persisted to disk.
#[instrument(skip(vfs))]
pub async fn run(vfs: &VfsRouter) {
    // System defaults — serialised from typed structs (no hard-coded YAML strings)
    let agent_defaults_yaml =
        DefaultsFile::from_agent_defaults(DefaultsLayer::System, None, &system_agent_defaults())
            .expect("phase1: serialise system agent defaults");

    vfs.write(
        &VfsPath::parse("/kernel/defaults/agent-manifest.yaml").unwrap(),
        agent_defaults_yaml.into_bytes(),
    )
    .await
    .expect("phase1: write /kernel/defaults/agent-manifest.yaml");

    // System limits — serialised from typed structs
    let agent_limits_yaml =
        LimitsFile::from_agent_limits(LimitsLayer::System, None, &system_agent_limits())
            .expect("phase1: serialise system agent limits");

    vfs.write(
        &VfsPath::parse("/kernel/limits/agent-manifest.yaml").unwrap(),
        agent_limits_yaml.into_bytes(),
    )
    .await
    .expect("phase1: write /kernel/limits/agent-manifest.yaml");

    // Anchor /proc/spawn-errors/ so the directory is listable
    vfs.write(
        &VfsPath::parse("/proc/spawn-errors/.keep").unwrap(),
        b"".to_vec(),
    )
    .await
    .expect("phase1: write /proc/spawn-errors anchor");

    // Runtime state directories for memory.svc
    vfs.ensure_dir(&VfsPath::parse("/proc/services/memory/").unwrap())
        .await
        .expect("phase1: create /proc/services/memory/");
    vfs.ensure_dir(&VfsPath::parse("/proc/services/memory/agents/").unwrap())
        .await
        .expect("phase1: create /proc/services/memory/agents/");

    tracing::info!("phase1: VFS skeleton initialised");
}
