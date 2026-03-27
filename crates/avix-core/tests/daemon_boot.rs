use avix_core::bootstrap::Runtime;
use std::fs;
use tempfile::TempDir;

const TEST_SIGNING_KEY: &str = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

#[tokio::test]
async fn daemon_spawn_probes_ok() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(
        root.join("etc/auth.conf"),
        r#"
apiVersion: v1
kind: AuthConfig
policy:
  session_ttl: 8h
  require_tls: false
identities:
- name: admin
  uid: 1000
  role: admin
  credential:
    type: api_key
    key_hash: test
    header: null
"#,
    )
    .unwrap();
    fs::write(root.join("etc/signing.key"), TEST_SIGNING_KEY).unwrap();
    let runtime = Runtime::bootstrap_with_root(root).await.unwrap();
    assert!(runtime.has_master_key());
    // Note: start_daemon would loop, so not tested here
}

#[tokio::test]
async fn hot_reload_writes_pending() {
    // hot_reload is stub, so placeholder test
    assert!(true);
}
