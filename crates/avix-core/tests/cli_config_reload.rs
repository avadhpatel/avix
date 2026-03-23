use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
use avix_core::cli::config_reload::{run_config_reload, ReloadParams};
use tempfile::tempdir;

async fn setup_avix_root() -> tempfile::TempDir {
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

// T-E-06: reload --check validates config and reports hot-reloadable sections
#[tokio::test]
async fn config_reload_check_reports_sections() {
    let dir = setup_avix_root().await;
    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    })
    .await
    .unwrap();
    assert!(
        result.reloaded_sections.contains(&"scheduler".to_string()),
        "scheduler should be in reloaded_sections"
    );
    assert!(
        result
            .reloaded_sections
            .contains(&"observability".to_string()),
        "observability should be in reloaded_sections"
    );
    assert!(
        result.restart_required.is_empty(),
        "nothing changed — restart_required should be empty, got {:?}",
        result.restart_required
    );
}

// T-E-07: reload --check reports restart-required when ipc changed
#[tokio::test]
async fn config_reload_check_detects_ipc_change() {
    let dir = setup_avix_root().await;
    let kernel_path = dir.path().join("etc/kernel.yaml");
    let yaml = std::fs::read_to_string(&kernel_path).unwrap();
    // Change ipc.timeoutMs from default 5000 to 9999
    let modified = yaml.replace("timeoutMs: 5000", "timeoutMs: 9999");
    std::fs::write(&kernel_path, modified).unwrap();

    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    })
    .await
    .unwrap();
    assert!(
        result.restart_required.contains(&"ipc".to_string()),
        "ipc timeout changed — ipc should be in restart_required, got {:?}",
        result.restart_required
    );
}

// T-E-08: reload --check fails on invalid kernel.yaml
#[tokio::test]
async fn config_reload_check_fails_on_invalid_config() {
    let dir = setup_avix_root().await;
    let kernel_path = dir.path().join("etc/kernel.yaml");
    let yaml = std::fs::read_to_string(&kernel_path).unwrap();
    // tickMs: 0 is invalid
    let modified = yaml.replace("tickMs: 100", "tickMs: 0");
    std::fs::write(&kernel_path, modified).unwrap();

    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    })
    .await;
    assert!(result.is_err(), "invalid config should return an error");
}

// T-E-09: reload writes reload-pending marker file
#[tokio::test]
async fn config_reload_writes_marker_file() {
    let dir = setup_avix_root().await;
    run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: false,
    })
    .await
    .unwrap();
    assert!(
        dir.path().join("run/avix/reload-pending").exists(),
        "reload-pending marker file should exist"
    );
}
