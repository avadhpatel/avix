use crate::memfs::{MemFs, VfsPath};

/// Phase 1: Write the kernel VFS skeleton.
///
/// Creates directory anchors and compiles-in default/limit files so that
/// agents spawned later can read system defaults from `/kernel/defaults/`.
/// All paths written here are kernel-owned ephemeral trees — they are
/// re-created on every boot, never persisted to disk.
pub async fn run(memfs: &MemFs) {
    memfs
        .write(
            &VfsPath::parse("/kernel/defaults/agent.yaml").unwrap(),
            AGENT_DEFAULTS_YAML.as_bytes().to_vec(),
        )
        .await
        .expect("phase1: write /kernel/defaults/agent.yaml");

    memfs
        .write(
            &VfsPath::parse("/kernel/defaults/pipe.yaml").unwrap(),
            PIPE_DEFAULTS_YAML.as_bytes().to_vec(),
        )
        .await
        .expect("phase1: write /kernel/defaults/pipe.yaml");

    memfs
        .write(
            &VfsPath::parse("/kernel/limits/agent.yaml").unwrap(),
            AGENT_LIMITS_YAML.as_bytes().to_vec(),
        )
        .await
        .expect("phase1: write /kernel/limits/agent.yaml");

    // Anchor /proc/spawn-errors/ so the directory is listable
    memfs
        .write(
            &VfsPath::parse("/proc/spawn-errors/.keep").unwrap(),
            b"".to_vec(),
        )
        .await
        .expect("phase1: write /proc/spawn-errors anchor");

    tracing::info!("phase1: VFS skeleton initialised");
}

// ── Compiled-in defaults ──────────────────────────────────────────────────────

const AGENT_DEFAULTS_YAML: &str = r#"apiVersion: avix/v1
kind: AgentDefaults
spec:
  contextWindowTokens: 64000
  maxToolChainLength: 50
  tokenTtlSecs: 3600
  renewalWindowSecs: 300
"#;

const PIPE_DEFAULTS_YAML: &str = r#"apiVersion: avix/v1
kind: PipeDefaults
spec:
  bufferTokens: 8192
  direction: out
"#;

const AGENT_LIMITS_YAML: &str = r#"apiVersion: avix/v1
kind: AgentLimits
spec:
  maxContextWindowTokens: 200000
  maxToolChainLength: 200
  maxConcurrentAgents: 100
"#;
