use avix_core::config::{AuthConfig, CrewsConfig, KernelConfig, LlmConfig, UsersConfig};
use avix_core::types::Modality;

// ─── AuthConfig tests ────────────────────────────────────────────────────────

fn auth_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 1h
  require_tls: true
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: api_key
      key_hash: "hmac-sha256:abc123"
  - name: bob
    uid: 1002
    role: user
    credential:
      type: password
      password_hash: "bcrypt:xyz"
"#
}

#[test]
fn auth_config_parses_successfully() {
    let cfg = AuthConfig::from_str(auth_yaml()).unwrap();
    assert_eq!(cfg.kind, "AuthConfig");
    assert_eq!(cfg.identities.len(), 2);
}

#[test]
fn auth_config_identity_roles() {
    let cfg = AuthConfig::from_str(auth_yaml()).unwrap();
    let alice = cfg.identities.iter().find(|i| i.name == "alice").unwrap();
    assert_eq!(alice.role.to_string(), "admin");
    let bob = cfg.identities.iter().find(|i| i.name == "bob").unwrap();
    assert_eq!(bob.role.to_string(), "user");
}

#[test]
fn auth_config_requires_at_least_one_admin() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 1h
identities:
  - name: bob
    uid: 1002
    role: user
    credential:
      type: api_key
      key_hash: "hash"
"#;
    assert!(AuthConfig::from_str(yaml).is_err());
}

#[test]
fn auth_config_rejects_empty_identities() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 1h
identities: []
"#;
    assert!(AuthConfig::from_str(yaml).is_err());
}

#[test]
fn auth_config_api_version() {
    let cfg = AuthConfig::from_str(auth_yaml()).unwrap();
    assert_eq!(cfg.api_version, "avix/v1");
}

// ─── KernelConfig tests ──────────────────────────────────────────────────────

fn kernel_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  ipc:
    transport: local-ipc
    socket_name: avix-kernel
"#
}

#[test]
fn kernel_config_parses_successfully() {
    let cfg = KernelConfig::from_str(kernel_yaml()).unwrap();
    assert_eq!(cfg.kind, "KernelConfig");
}

#[test]
fn kernel_config_ipc_transport() {
    use avix_core::config::IpcTransportKind;
    let cfg = KernelConfig::from_str(kernel_yaml()).unwrap();
    assert_eq!(cfg.spec.ipc.transport, IpcTransportKind::LocalIpc);
}

#[test]
fn kernel_config_socket_name() {
    let cfg = KernelConfig::from_str(kernel_yaml()).unwrap();
    assert_eq!(cfg.spec.ipc.socket_name, "avix-kernel");
}

// ─── UsersConfig tests ───────────────────────────────────────────────────────

fn users_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: UsersConfig
users:
  - name: alice
    uid: 1001
    role: admin
    additionalTools:
      - "exec/python"
    deniedTools: []
    quota:
      tokens: 1000000
      requestsPerDay: 500
  - name: bob
    uid: 1002
    role: user
"#
}

#[test]
fn users_config_parses_successfully() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    assert_eq!(cfg.users.len(), 2);
}

#[test]
fn users_config_additional_tools() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    let alice = cfg.users.iter().find(|u| u.name == "alice").unwrap();
    assert!(alice.additional_tools.contains(&"exec/python".to_string()));
}

#[test]
fn users_config_quota() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    let alice = cfg.users.iter().find(|u| u.name == "alice").unwrap();
    let quota = alice.quota.as_ref().unwrap();
    assert_eq!(quota.tokens, Some(1_000_000));
    assert_eq!(quota.requests_per_day, Some(500));
}

#[test]
fn users_config_rejects_duplicate_uids() {
    let yaml = r#"
apiVersion: avix/v1
kind: UsersConfig
users:
  - name: alice
    uid: 1001
    role: admin
  - name: bob
    uid: 1001
    role: user
"#;
    assert!(UsersConfig::from_str(yaml).is_err());
}

// ─── CrewsConfig tests ───────────────────────────────────────────────────────

fn crews_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: CrewsConfig
crews:
  - cid: research-crew
    members:
      - alice
      - bob
    allowedTools:
      - "fs/read"
      - "llm/complete"
    deniedTools:
      - "exec/shell"
"#
}

#[test]
fn crews_config_parses_successfully() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    assert_eq!(cfg.crews.len(), 1);
}

#[test]
fn crews_config_members() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    let crew = &cfg.crews[0];
    assert_eq!(crew.cid, "research-crew");
    assert!(crew.members.contains(&"alice".to_string()));
    assert!(crew.members.contains(&"bob".to_string()));
}

#[test]
fn crews_config_allowed_denied_tools() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    let crew = &cfg.crews[0];
    assert!(crew.allowed_tools.contains(&"fs/read".to_string()));
    assert!(crew.denied_tools.contains(&"exec/shell".to_string()));
}

// ─── LlmConfig tests ─────────────────────────────────────────────────────────

fn llm_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: openai
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com/v1
      modalities:
        - text
        - image
        - speech
        - transcription
        - embedding
      auth:
        type: api_key
        secretName: openai-api-key
        header: Authorization
"#
}

#[test]
fn llm_config_parses_successfully() {
    let cfg = LlmConfig::from_str(llm_yaml()).unwrap();
    assert_eq!(cfg.kind, "LlmConfig");
    assert_eq!(cfg.spec.providers.len(), 1);
}

#[test]
fn llm_config_default_providers() {
    let cfg = LlmConfig::from_str(llm_yaml()).unwrap();
    assert_eq!(cfg.spec.default_providers.text, "openai");
    assert_eq!(cfg.spec.default_providers.image, "openai");
}

#[test]
fn llm_config_provider_modalities() {
    let cfg = LlmConfig::from_str(llm_yaml()).unwrap();
    let openai = &cfg.spec.providers[0];
    assert!(openai.modalities.contains(&Modality::Text));
    assert!(openai.modalities.contains(&Modality::Image));
}

#[test]
fn llm_config_default_provider_for() {
    let cfg = LlmConfig::from_str(llm_yaml()).unwrap();
    let provider = cfg.default_provider_for(Modality::Text).unwrap();
    assert_eq!(provider.name, "openai");
}

#[test]
fn llm_config_fails_when_default_provider_not_found() {
    let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: missing-provider
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com/v1
      modalities:
        - image
        - speech
        - transcription
        - embedding
      auth:
        type: none
"#;
    assert!(LlmConfig::from_str(yaml).is_err());
}

#[test]
fn llm_config_fails_when_modality_not_supported() {
    let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: openai
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com/v1
      modalities:
        - image
        - speech
        - transcription
        - embedding
      auth:
        type: none
"#;
    assert!(LlmConfig::from_str(yaml).is_err());
}

// ── Finding C: config init writes all /etc/avix/ files ───────────────────────

use avix_core::cli::config_init::{ConfigInitParams, run_config_init};
use tempfile::tempdir;

fn make_params(tmp: &std::path::Path, identity: &str, role: &str) -> ConfigInitParams {
    ConfigInitParams {
        root: tmp.to_path_buf(),
        identity_name: identity.into(),
        credential_type: "api_key".into(),
        role: role.into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }
}

#[test]
fn config_init_creates_kernel_yaml() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let path = tmp.path().join("etc/kernel.yaml");
    assert!(path.exists(), "kernel.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains("KernelConfig"),
        "kernel.yaml must have kind: KernelConfig"
    );
    assert!(
        content.contains("AVIX_MASTER_KEY"),
        "kernel.yaml must reference AVIX_MASTER_KEY"
    );
}

#[test]
fn config_init_creates_users_yaml_with_identity() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "bob", "user")).unwrap();

    let content = std::fs::read_to_string(tmp.path().join("etc/users.yaml")).unwrap();
    assert!(content.contains("bob"), "users.yaml must contain the identity name");
    assert!(content.contains("user"), "users.yaml must contain the role");
    assert!(
        content.contains("UsersConfig") || content.contains("Users"),
        "users.yaml must have kind: Users or UsersConfig"
    );
}

#[test]
fn config_init_creates_crews_yaml() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let path = tmp.path().join("etc/crews.yaml");
    assert!(path.exists(), "crews.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("Crews"), "crews.yaml must have kind: Crews");
}

#[test]
fn config_init_creates_crontab_yaml() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let path = tmp.path().join("etc/crontab.yaml");
    assert!(path.exists(), "crontab.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(
        content.contains("Crontab"),
        "crontab.yaml must have kind: Crontab"
    );
}

#[test]
fn config_init_creates_fstab_yaml_with_local_mounts() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let path = tmp.path().join("etc/fstab.yaml");
    assert!(path.exists(), "fstab.yaml must exist after config init");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("Fstab"), "fstab.yaml must have kind: Fstab");
    assert!(
        content.contains("local"),
        "fstab.yaml must define at least one local mount"
    );
    assert!(
        content.contains("/etc/avix") || content.contains("etc"),
        "fstab.yaml must mount the etc/avix tree"
    );
    assert!(content.contains("/secrets"), "fstab.yaml must mount /secrets");
}

#[test]
fn config_init_all_files_idempotent_without_force() {
    let tmp = tempdir().unwrap();

    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();
    let mtime1 = std::fs::metadata(tmp.path().join("etc/kernel.yaml"))
        .unwrap()
        .modified()
        .unwrap();

    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();
    let mtime2 = std::fs::metadata(tmp.path().join("etc/kernel.yaml"))
        .unwrap()
        .modified()
        .unwrap();

    assert_eq!(
        mtime1, mtime2,
        "kernel.yaml must not be rewritten on second config init without --force"
    );
}

#[test]
fn config_init_creates_data_dirs_for_mounts() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    assert!(
        tmp.path().join("data/users/alice").exists(),
        "data/users/<identity> directory must be created at config init"
    );
    assert!(
        tmp.path().join("secrets").exists(),
        "secrets directory must be created at config init"
    );
}
