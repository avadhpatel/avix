use avix_core::bootstrap::Runtime;
use avix_core::memfs::VfsPath;
use serial_test::serial;
use std::time::Instant;
use tempfile::tempdir;

fn write_minimal_auth_conf(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("etc")).unwrap();
    std::fs::write(
        root.join("etc/auth.conf"),
        r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: api_key
      key_hash: "hmac-sha256:test"
"#,
    )
    .unwrap();
}

#[tokio::test]
#[serial]
async fn bootstrap_aborts_without_auth_conf() {
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    std::env::remove_var("AVIX_MASTER_KEY");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("auth.conf"));
}

#[tokio::test]
#[serial]
async fn bootstrap_succeeds_with_valid_auth_conf() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    std::env::remove_var("AVIX_MASTER_KEY");
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn phase2_loads_master_key_from_env() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    assert!(runtime.has_master_key());
    assert!(std::env::var("AVIX_MASTER_KEY").is_err());
}

#[tokio::test]
#[serial]
async fn phase2_fails_without_master_key_env_var() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::remove_var("AVIX_MASTER_KEY");
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("AVIX_MASTER_KEY"));
}

#[tokio::test]
#[serial]
async fn bootstrap_phases_complete_in_order() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    let log = runtime.boot_log();
    let phases: Vec<_> = log.iter().map(|e| e.phase).collect();
    assert!(phases.windows(2).all(|w| w[0] < w[1]));
}

#[tokio::test]
#[serial]
async fn bootstrap_completes_within_700ms() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let start = Instant::now();
    Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    assert!(start.elapsed().as_millis() < 700);
}

#[tokio::test]
#[serial]
async fn built_in_services_get_low_pids() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    let router_pid = runtime.service_pid("router").unwrap();
    assert!(
        router_pid.as_u32() <= 9,
        "router should have PID ≤ 9, got {}",
        router_pid
    );
}

#[tokio::test]
#[serial]
async fn llm_service_pid_present_after_bootstrap() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    let llm_pid = runtime.service_pid("llm");
    assert!(
        llm_pid.is_some(),
        "expected llm service to have a PID after bootstrap"
    );
}

// ── Finding A: Phase 1 VFS tree initialization ────────────────────────────────

#[tokio::test]
#[serial]
async fn phase1_creates_proc_directory_anchor() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let result = runtime.vfs().list(&VfsPath::parse("/proc").unwrap()).await;
    assert!(
        result.is_ok(),
        "/proc should exist after Phase 1: {:?}",
        result
    );
}

#[tokio::test]
#[serial]
async fn phase1_creates_kernel_defaults_agent_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/defaults/agent.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/defaults/agent.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("contextWindowTokens"),
        "agent defaults must include contextWindowTokens"
    );
    assert!(
        text.contains("maxToolChainLength"),
        "agent defaults must include maxToolChainLength"
    );
}

#[tokio::test]
#[serial]
async fn phase1_creates_kernel_defaults_pipe_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/defaults/pipe.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/defaults/pipe.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("bufferTokens"),
        "pipe defaults must include bufferTokens"
    );
}

#[tokio::test]
#[serial]
async fn phase1_creates_kernel_limits_agent_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let path = VfsPath::parse("/kernel/limits/agent.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/limits/agent.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("maxContextWindowTokens"),
        "limits must include maxContextWindowTokens"
    );
}

#[tokio::test]
#[serial]
async fn phase1_creates_spawn_errors_directory() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let sentinel = VfsPath::parse("/proc/spawn-errors/.keep").unwrap();
    let write_result = runtime.vfs().write(&sentinel, b"".to_vec()).await;
    assert!(
        write_result.is_ok(),
        "/proc/spawn-errors/ must be navigable after Phase 1"
    );
}

#[tokio::test]
#[serial]
async fn phase1_runs_before_phase2() {
    let tmp = tempdir().unwrap();
    write_minimal_auth_conf(tmp.path());
    std::env::set_var("AVIX_MASTER_KEY", "test_key_32_bytes_exactly_here!!");
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    std::env::remove_var("AVIX_MASTER_KEY");

    let log = runtime.boot_log();
    let phase1_idx = log.iter().position(|e| e.phase.0 == 1).unwrap();
    let phase2_idx = log.iter().position(|e| e.phase.0 == 2).unwrap();
    assert!(
        phase1_idx < phase2_idx,
        "Phase 1 must complete before Phase 2"
    );
}
