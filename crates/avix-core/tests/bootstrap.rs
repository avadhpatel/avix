use avix_core::bootstrap::Runtime;
use avix_core::memfs::VfsPath;
use serial_test::serial;
use std::time::Instant;
use tempfile::tempdir;

const TEST_SIGNING_KEY: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

fn write_minimal_server_config(root: &std::path::Path) {
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
    std::fs::write(root.join("etc/signing.key"), TEST_SIGNING_KEY).unwrap();
}

#[tokio::test]
#[serial]
async fn bootstrap_aborts_without_auth_conf() {
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
    std::fs::write(tmp.path().join("etc/signing.key"), TEST_SIGNING_KEY).unwrap();
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("auth.conf"));
}

#[tokio::test]
#[serial]
async fn bootstrap_succeeds_with_valid_auth_conf() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[serial]
async fn phase2_loads_signing_key_from_file() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    assert!(runtime.has_master_key());
}

#[tokio::test]
#[serial]
async fn phase2_fails_without_signing_key() {
    let tmp = tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("etc")).unwrap();
    std::fs::write(
        tmp.path().join("etc/auth.conf"),
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
    // signing.key intentionally absent
    let result = Runtime::bootstrap_with_root(tmp.path()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("signing.key"));
}

#[tokio::test]
#[serial]
async fn bootstrap_phases_complete_in_order() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    let log = runtime.boot_log();
    let phases: Vec<_> = log.iter().map(|e| e.phase).collect();
    assert!(phases.windows(2).all(|w| w[0] < w[1]));
}

#[tokio::test]
#[serial]
async fn bootstrap_completes_within_700ms() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let start = Instant::now();
    Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    assert!(start.elapsed().as_millis() < 700);
}

/// Built-in service PIDs are assigned when `start_daemon` calls `phase3_services`,
/// not during `bootstrap_with_root`.  At bootstrap time the map is empty.
#[tokio::test]
#[serial]
async fn service_pid_is_none_before_daemon_start() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();
    // Services haven't been started yet — all return None.
    assert!(runtime.service_pid("router").is_none());
    assert!(runtime.service_pid("llm").is_none());
    assert!(runtime.service_pid("exec").is_none());
}

// ── Finding A: Phase 1 VFS tree initialization ────────────────────────────────

#[tokio::test]
#[serial]
async fn phase1_creates_proc_directory_anchor() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

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
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

    let path = VfsPath::parse("/kernel/defaults/agent-manifest.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/defaults/agent-manifest.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("maxToolChain"),
        "agent defaults must include maxToolChain"
    );
    assert!(
        text.contains("modelPreference"),
        "agent defaults must include modelPreference"
    );
}

#[tokio::test]
#[serial]
async fn phase1_creates_kernel_limits_agent_yaml() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

    let path = VfsPath::parse("/kernel/limits/agent-manifest.yaml").unwrap();
    assert!(
        runtime.vfs().exists(&path).await,
        "/kernel/limits/agent-manifest.yaml must be written at boot"
    );
    let content = runtime.vfs().read(&path).await.unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("maxToolChain"),
        "limits must include maxToolChain"
    );
    assert!(
        text.contains("temperature"),
        "limits must include temperature"
    );
}

#[tokio::test]
#[serial]
async fn phase1_defaults_round_trips_through_typed_structs() {
    use avix_core::params::{DefaultsFile, LimitsFile};

    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

    let raw = runtime
        .vfs()
        .read(&VfsPath::parse("/kernel/defaults/agent-manifest.yaml").unwrap())
        .await
        .unwrap();
    let file = DefaultsFile::from_str(&String::from_utf8(raw).unwrap()).unwrap();
    let d = file.as_agent_defaults().unwrap();
    assert_eq!(d.entrypoint.as_ref().unwrap().max_tool_chain, Some(5));

    let raw = runtime
        .vfs()
        .read(&VfsPath::parse("/kernel/limits/agent-manifest.yaml").unwrap())
        .await
        .unwrap();
    let file = LimitsFile::from_str(&String::from_utf8(raw).unwrap()).unwrap();
    let l = file.as_agent_limits().unwrap();
    assert!(l.entrypoint.as_ref().unwrap().max_tool_chain.is_some());
}

#[tokio::test]
#[serial]
async fn phase1_creates_spawn_errors_directory() {
    let tmp = tempdir().unwrap();
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

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
    write_minimal_server_config(tmp.path());
    let runtime = Runtime::bootstrap_with_root(tmp.path()).await.unwrap();

    let log = runtime.boot_log();
    let phase1_idx = log.iter().position(|e| e.phase.0 == 1).unwrap();
    let phase2_idx = log.iter().position(|e| e.phase.0 == 2).unwrap();
    assert!(
        phase1_idx < phase2_idx,
        "Phase 1 must complete before Phase 2"
    );
}
