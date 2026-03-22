use avix_core::bootstrap::Runtime;
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
