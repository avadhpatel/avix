use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
use avix_core::cli::resolve::{run_resolve, ResolveParams};
use avix_core::params::constraint::RangeConstraint;
use avix_core::params::limits::{AgentLimits, EntrypointLimits, LimitsFile, LimitsLayer};
use avix_core::params::resolved_file::ResolvedFile;
use std::path::PathBuf;
use tempfile::tempdir;

async fn setup_avix_root_with_alice() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: dir.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    })
    .unwrap();
    dir
}

fn base_resolve_params(root: PathBuf) -> ResolveParams {
    ResolveParams {
        root,
        kind: "agent-manifest".into(),
        username: "alice".into(),
        agent_name: None,
        explain: false,
        limits_only: false,
        extra_crew: None,
        dry_run: false,
    }
}

// T-E-01: avix resolve returns valid Resolved YAML
#[tokio::test]
async fn resolve_returns_valid_yaml() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(base_resolve_params(dir.path().to_path_buf()))
        .await
        .unwrap();
    let parsed = ResolvedFile::from_str(&result.output).unwrap();
    assert_eq!(parsed.metadata.resolved_for.username, "alice");
    assert_eq!(parsed.kind, "Resolved");
}

// T-E-02: --explain includes annotations block
#[tokio::test]
async fn resolve_explain_includes_annotations() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        explain: true,
        ..base_resolve_params(dir.path().to_path_buf())
    })
    .await
    .unwrap();
    let parsed = ResolvedFile::from_str(&result.output).unwrap();
    assert!(parsed.annotations.is_some());
    assert!(!parsed.annotations.unwrap().is_empty());
}

// T-E-03: --limits-only returns only effective limits YAML
#[tokio::test]
async fn resolve_limits_only_returns_limits() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        limits_only: true,
        ..base_resolve_params(dir.path().to_path_buf())
    })
    .await
    .unwrap();
    assert!(result.output.contains("kind: Limits"));
}

// T-E-04: --crew simulates additional crew membership with tighter limits
#[tokio::test]
async fn resolve_dry_run_crew_simulation() {
    let dir = setup_avix_root_with_alice_and_automation_crew().await;
    let result = run_resolve(ResolveParams {
        extra_crew: Some("automation".into()),
        dry_run: true,
        ..base_resolve_params(dir.path().to_path_buf())
    })
    .await
    .unwrap();
    let parsed = ResolvedFile::from_str(&result.output).unwrap();
    // automation crew limits max_tool_chain to 4; system default is 5 → clamped to 4
    assert!(
        parsed.resolved.entrypoint.max_tool_chain <= 4,
        "expected max_tool_chain <= 4, got {}",
        parsed.resolved.entrypoint.max_tool_chain
    );
}

// T-E-05: Unknown user returns error
#[tokio::test]
async fn resolve_unknown_user_returns_error() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        username: "nonexistent".into(),
        ..base_resolve_params(dir.path().to_path_buf())
    })
    .await;
    assert!(result.is_err());
}

// Helper: avix root with alice + automation crew that has tight limits
async fn setup_avix_root_with_alice_and_automation_crew() -> tempfile::TempDir {
    let dir = setup_avix_root_with_alice().await;

    let crew_limits = AgentLimits {
        entrypoint: Some(EntrypointLimits {
            max_tool_chain: Some(RangeConstraint {
                min: None,
                max: Some(4.0),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let yaml =
        LimitsFile::from_agent_limits(LimitsLayer::Crew, Some("automation".into()), &crew_limits)
            .unwrap();

    let crew_dir = dir.path().join("data/crews/automation");
    std::fs::create_dir_all(&crew_dir).unwrap();
    std::fs::write(crew_dir.join("limits.yaml"), yaml).unwrap();

    dir
}
