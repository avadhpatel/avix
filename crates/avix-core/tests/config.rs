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
    socketName: avix-kernel
"#
}

fn kernel_yaml_full() -> &'static str {
    r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  scheduler:
    algorithm: priority_deadline
    tickMs: 100
    preemption: true
    maxConcurrentAgents: 50
  memory:
    defaultContextLimit: 200000
    episodic:
      maxRetentionDays: 30
      maxRecordsPerAgent: 10000
    retrieval:
      defaultLimit: 5
      maxLimit: 20
  ipc:
    transport: local-ipc
    socketName: avix-kernel
    maxMessageBytes: 65536
    timeoutMs: 5000
  safety:
    policyEngine: enabled
    hilOnEscalation: true
    maxToolChainLength: 10
    blockedToolChains: []
  models:
    default: claude-sonnet-4
    kernel: claude-opus-4
    fallback: claude-haiku-4
    temperature: 0.7
  observability:
    logLevel: info
    logPath: /var/log/avix/kernel.log
    metricsEnabled: true
    metricsPath: /var/log/avix/metrics/
    traceEnabled: false
  secrets:
    algorithm: aes-256-gcm
    masterKey:
      source: env
      envVar: AVIX_MASTER_KEY
    store:
      path: /secrets
      provider: local
    audit:
      enabled: true
      logPath: /var/log/avix/secrets-audit.log
      logReads: true
      logWrites: true
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

#[test]
fn kernel_config_full_parse() {
    use avix_core::config::{LogLevel, SchedulerAlgorithm};
    let cfg = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    assert_eq!(
        cfg.spec.scheduler.algorithm,
        SchedulerAlgorithm::PriorityDeadline
    );
    assert_eq!(cfg.spec.scheduler.tick_ms, 100);
    assert_eq!(cfg.spec.memory.default_context_limit, 200_000);
    assert_eq!(cfg.spec.memory.episodic.max_retention_days, 30);
    assert_eq!(cfg.spec.memory.retrieval.default_limit, 5);
    assert_eq!(cfg.spec.safety.max_tool_chain_length, 10);
    assert!(cfg.spec.safety.hil_on_escalation);
    assert!(!cfg.spec.observability.trace_enabled);
    assert_eq!(cfg.spec.observability.log_level, LogLevel::Info);
    assert!((cfg.spec.models.temperature - 0.7).abs() < f32::EPSILON);
    assert_eq!(cfg.spec.models.default, "claude-sonnet-4");
    assert_eq!(cfg.spec.models.kernel, "claude-opus-4");
}

#[test]
fn kernel_config_defaults_applied_for_missing_fields() {
    // Minimal YAML — all sections default
    let minimal = "apiVersion: avix/v1\nkind: KernelConfig\n";
    let cfg = KernelConfig::from_str(minimal).unwrap();
    assert_eq!(cfg.spec.scheduler.tick_ms, 100);
    assert_eq!(cfg.spec.scheduler.max_concurrent_agents, 50);
    assert!(cfg.spec.safety.hil_on_escalation);
    assert_eq!(cfg.spec.ipc.max_message_bytes, 65_536);
    assert!(!cfg.spec.observability.trace_enabled);
}

// T-MA-08: MemoryConfig defaults match spec
#[test]
fn memory_config_defaults() {
    use avix_core::config::MemoryConfig;
    let cfg: MemoryConfig = serde_yaml::from_str("{}").unwrap();
    assert_eq!(cfg.default_context_limit, 200_000);
    assert_eq!(cfg.episodic.max_retention_days, 30);
    assert_eq!(cfg.episodic.max_records_per_agent, 10_000);
    assert_eq!(cfg.semantic.max_facts_per_agent, 5_000);
    assert_eq!(cfg.retrieval.default_limit, 5);
    assert_eq!(cfg.retrieval.max_limit, 20);
    assert_eq!(cfg.retrieval.rrf_k, 60);
    assert!(cfg.sharing.enabled);
    assert!(!cfg.sharing.cross_user_enabled);
    assert_eq!(cfg.sharing.hil_timeout_sec, 600);
}

#[test]
fn kernel_config_validate_passes_for_valid_config() {
    let cfg = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    cfg.validate().unwrap();
}

#[test]
fn kernel_config_validate_rejects_zero_tick_ms() {
    let yaml = r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  scheduler:
    tickMs: 0
"#;
    let cfg = KernelConfig::from_str(yaml).unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn kernel_config_validate_rejects_bad_temperature() {
    let yaml = r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  models:
    temperature: 3.0
"#;
    let cfg = KernelConfig::from_str(yaml).unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn kernel_config_validate_rejects_low_max_message_bytes() {
    let yaml = r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  ipc:
    maxMessageBytes: 100
"#;
    let cfg = KernelConfig::from_str(yaml).unwrap();
    assert!(cfg.validate().is_err());
}

#[test]
fn kernel_config_requires_restart_for_ipc_change() {
    let a = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    let mut b = a.clone();
    b.spec.ipc.max_message_bytes = 131_072;
    assert!(a.requires_restart(&b));
}

#[test]
fn kernel_config_no_restart_for_observability_change() {
    use avix_core::config::LogLevel;
    let a = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    let mut b = a.clone();
    b.spec.observability.log_level = LogLevel::Debug;
    assert!(!a.requires_restart(&b));
}

#[test]
fn kernel_config_requires_restart_for_kernel_model_change() {
    let a = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    let mut b = a.clone();
    b.spec.models.kernel = "claude-haiku-4".into();
    assert!(a.requires_restart(&b));
}

#[test]
fn kernel_config_no_restart_for_safety_change() {
    let a = KernelConfig::from_str(kernel_yaml_full()).unwrap();
    let mut b = a.clone();
    b.spec.safety.max_tool_chain_length = 20;
    assert!(!a.requires_restart(&b));
}

// ─── UsersConfig tests ───────────────────────────────────────────────────────

fn users_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: Users
metadata:
  lastUpdated: "2026-03-20T00:00:00Z"
spec:
  users:
    - username: alice
      uid: 1001
      workspace: /users/alice/workspace
      shell: /bin/sh
      crews: [researchers]
      additionalTools:
        - "exec/python"
      deniedTools: []
      quota:
        tokens: 1000000
        agents: 5
        sessions: 4
    - username: bob
      uid: 2001
      workspace: /users/bob/workspace
      shell: /bin/sh
      crews: []
"#
}

#[test]
fn users_config_parses_successfully() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    assert_eq!(cfg.users().len(), 2);
}

#[test]
fn users_config_find_user() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    assert!(cfg.find_user("alice").is_some());
    assert!(cfg.find_user("nobody").is_none());
}

#[test]
fn users_config_additional_tools() {
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    let alice = cfg.find_user("alice").unwrap();
    assert!(alice.additional_tools.contains(&"exec/python".to_string()));
}

#[test]
fn users_config_quota() {
    use avix_core::config::QuotaValue;
    let cfg = UsersConfig::from_str(users_yaml()).unwrap();
    let alice = cfg.find_user("alice").unwrap();
    let quota = alice.quota.as_ref().unwrap();
    assert_eq!(quota.tokens, Some(QuotaValue::Count(1_000_000)));
    assert_eq!(quota.agents, Some(QuotaValue::Count(5)));
}

#[test]
fn users_config_rejects_duplicate_uids() {
    let yaml = r#"
apiVersion: avix/v1
kind: Users
spec:
  users:
    - username: alice
      uid: 1001
    - username: bob
      uid: 1001
"#;
    assert!(UsersConfig::from_str(yaml).is_err());
}

#[test]
fn users_config_rejects_reserved_uid_range() {
    let yaml =
        "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n  - username: svc\n    uid: 500\n";
    assert!(UsersConfig::from_str(yaml).is_err());
}

// ─── CrewsConfig tests ───────────────────────────────────────────────────────

fn crews_yaml() -> &'static str {
    r#"
apiVersion: avix/v1
kind: Crews
metadata:
  lastUpdated: "2026-03-20T00:00:00Z"
spec:
  crews:
    - name: researchers
      cid: 1001
      members:
        - user:alice
        - user:bob
      allowedTools:
        - "fs/read"
        - "llm/complete"
      deniedTools:
        - "exec/shell"
      sharedPaths:
        - /crews/researchers/shared/docs/
"#
}

#[test]
fn crews_config_parses_successfully() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    assert_eq!(cfg.crews().len(), 1);
}

#[test]
fn crews_config_find_crew() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    assert!(cfg.find_crew("researchers").is_some());
    assert!(cfg.find_crew("nobody").is_none());
}

#[test]
fn crews_config_members() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    let crew = cfg.find_crew("researchers").unwrap();
    assert_eq!(crew.cid, 1001);
    assert!(crew.contains_user("alice"));
    assert!(crew.contains_user("bob"));
}

#[test]
fn crews_config_allowed_denied_tools() {
    let cfg = CrewsConfig::from_str(crews_yaml()).unwrap();
    let crew = cfg.find_crew("researchers").unwrap();
    assert!(crew.allowed_tools.contains(&"fs/read".to_string()));
    assert!(crew.denied_tools.contains(&"exec/shell".to_string()));
}

#[test]
fn crews_config_rejects_duplicate_cids() {
    let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
            - name: a\n    cid: 1001\n  - name: b\n    cid: 1001\n";
    assert!(CrewsConfig::from_str(yaml).is_err());
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

use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
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
    assert!(
        content.contains("bob"),
        "users.yaml must contain the identity name"
    );
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
    assert!(
        content.contains("Crews"),
        "crews.yaml must have kind: Crews"
    );
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
    assert!(
        content.contains("Fstab"),
        "fstab.yaml must have kind: Fstab"
    );
    assert!(
        content.contains("local"),
        "fstab.yaml must define at least one local mount"
    );
    assert!(
        content.contains("/etc/avix") || content.contains("etc"),
        "fstab.yaml must mount the etc/avix tree"
    );
    assert!(
        content.contains("/secrets"),
        "fstab.yaml must mount /secrets"
    );
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
fn config_init_kernel_yaml_has_all_sections() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let content = std::fs::read_to_string(tmp.path().join("etc/kernel.yaml")).unwrap();
    assert!(
        content.contains("scheduler:"),
        "kernel.yaml must have scheduler section"
    );
    assert!(
        content.contains("memory:"),
        "kernel.yaml must have memory section"
    );
    assert!(
        content.contains("safety:"),
        "kernel.yaml must have safety section"
    );
    assert!(
        content.contains("models:"),
        "kernel.yaml must have models section"
    );
    assert!(
        content.contains("observability:"),
        "kernel.yaml must have observability section"
    );
    assert!(
        content.contains("secrets:"),
        "kernel.yaml must have secrets section"
    );
}

#[test]
fn config_init_kernel_yaml_is_valid() {
    let tmp = tempdir().unwrap();
    run_config_init(make_params(tmp.path(), "alice", "admin")).unwrap();

    let raw = std::fs::read_to_string(tmp.path().join("etc/kernel.yaml")).unwrap();
    let cfg = KernelConfig::from_str(&raw).unwrap();
    cfg.validate().unwrap();
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
