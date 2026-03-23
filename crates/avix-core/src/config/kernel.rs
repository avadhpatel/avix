use crate::error::AvixError;
use serde::{Deserialize, Serialize};

// ── Scheduler ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerAlgorithm {
    #[default]
    PriorityDeadline,
    RoundRobin,
    Fifo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerConfig {
    #[serde(default = "SchedulerConfig::default_algorithm")]
    pub algorithm: SchedulerAlgorithm,
    #[serde(default = "SchedulerConfig::default_tick_ms")]
    pub tick_ms: u32,
    #[serde(default = "SchedulerConfig::default_preemption")]
    pub preemption: bool,
    #[serde(default = "SchedulerConfig::default_max_concurrent_agents")]
    pub max_concurrent_agents: u32,
}

impl SchedulerConfig {
    fn default_algorithm() -> SchedulerAlgorithm {
        SchedulerAlgorithm::PriorityDeadline
    }
    fn default_tick_ms() -> u32 {
        100
    }
    fn default_preemption() -> bool {
        true
    }
    fn default_max_concurrent_agents() -> u32 {
        50
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            algorithm: SchedulerAlgorithm::PriorityDeadline,
            tick_ms: 100,
            preemption: true,
            max_concurrent_agents: 50,
        }
    }
}

// ── Memory ───────────────────────────────────────────────────────────────────

fn default_context_limit() -> u32 {
    200_000
}
fn default_max_retention_days() -> u32 {
    30
}
fn default_max_records_per_agent() -> u32 {
    10_000
}
fn default_max_facts_per_agent() -> u32 {
    5_000
}
fn default_retrieval_limit() -> u32 {
    5
}
fn default_max_retrieval_limit() -> u32 {
    20
}
fn default_candidate_fetch_k() -> u32 {
    20
}
fn default_rrf_k() -> u32 {
    60
}
fn default_episodic_context_records() -> u32 {
    5
}
fn default_true() -> bool {
    true
}
fn default_hil_timeout_sec() -> u64 {
    600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEpisodicConfig {
    #[serde(default = "default_max_retention_days")]
    pub max_retention_days: u32,
    #[serde(default = "default_max_records_per_agent")]
    pub max_records_per_agent: u32,
}

impl Default for MemoryEpisodicConfig {
    fn default() -> Self {
        Self {
            max_retention_days: default_max_retention_days(),
            max_records_per_agent: default_max_records_per_agent(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySemanticConfig {
    #[serde(default = "default_max_facts_per_agent")]
    pub max_facts_per_agent: u32,
}

impl Default for MemorySemanticConfig {
    fn default() -> Self {
        Self {
            max_facts_per_agent: default_max_facts_per_agent(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRetrievalConfig {
    #[serde(default = "default_retrieval_limit")]
    pub default_limit: u32,
    #[serde(default = "default_max_retrieval_limit")]
    pub max_limit: u32,
    #[serde(default = "default_candidate_fetch_k")]
    pub candidate_fetch_k: u32,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
}

impl Default for MemoryRetrievalConfig {
    fn default() -> Self {
        Self {
            default_limit: default_retrieval_limit(),
            max_limit: default_max_retrieval_limit(),
            candidate_fetch_k: default_candidate_fetch_k(),
            rrf_k: default_rrf_k(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySpawnConfig {
    #[serde(default = "default_episodic_context_records")]
    pub episodic_context_records: u32,
    #[serde(default = "default_true")]
    pub preferences_enabled: bool,
    #[serde(default = "default_true")]
    pub pinned_facts_enabled: bool,
}

impl Default for MemorySpawnConfig {
    fn default() -> Self {
        Self {
            episodic_context_records: default_episodic_context_records(),
            preferences_enabled: true,
            pinned_facts_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySharingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hil_timeout_sec")]
    pub hil_timeout_sec: u64,
    /// Always `false` in v0.1 — cross-user memory sharing is not supported.
    #[serde(default)]
    pub cross_user_enabled: bool,
}

impl Default for MemorySharingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hil_timeout_sec: default_hil_timeout_sec(),
            cross_user_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    #[serde(default = "default_context_limit")]
    pub default_context_limit: u32,
    #[serde(default)]
    pub episodic: MemoryEpisodicConfig,
    #[serde(default)]
    pub semantic: MemorySemanticConfig,
    #[serde(default)]
    pub retrieval: MemoryRetrievalConfig,
    #[serde(default)]
    pub spawn: MemorySpawnConfig,
    #[serde(default)]
    pub sharing: MemorySharingConfig,
}

// ── IPC ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IpcTransportKind {
    #[default]
    LocalIpc,
    UnixSocket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcConfig {
    #[serde(default)]
    pub transport: IpcTransportKind,
    /// Logical socket name used by platform path resolver.
    #[serde(default = "IpcConfig::default_socket_name")]
    pub socket_name: String,
    #[serde(default = "IpcConfig::default_max_message_bytes")]
    pub max_message_bytes: u32,
    #[serde(default = "IpcConfig::default_timeout_ms")]
    pub timeout_ms: u32,
}

impl IpcConfig {
    fn default_socket_name() -> String {
        "avix-kernel".into()
    }
    fn default_max_message_bytes() -> u32 {
        65_536
    }
    fn default_timeout_ms() -> u32 {
        5_000
    }
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            transport: IpcTransportKind::LocalIpc,
            socket_name: "avix-kernel".into(),
            max_message_bytes: 65_536,
            timeout_ms: 5_000,
        }
    }
}

// ── Safety ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEngineMode {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedToolChain {
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyConfig {
    #[serde(default = "SafetyConfig::default_policy_engine")]
    pub policy_engine: PolicyEngineMode,
    #[serde(default = "SafetyConfig::default_hil_on_escalation")]
    pub hil_on_escalation: bool,
    #[serde(default = "SafetyConfig::default_max_tool_chain_length")]
    pub max_tool_chain_length: u32,
    #[serde(default)]
    pub blocked_tool_chains: Vec<BlockedToolChain>,
}

impl SafetyConfig {
    fn default_policy_engine() -> PolicyEngineMode {
        PolicyEngineMode::Enabled
    }
    fn default_hil_on_escalation() -> bool {
        true
    }
    fn default_max_tool_chain_length() -> u32 {
        10
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            policy_engine: PolicyEngineMode::Enabled,
            hil_on_escalation: true,
            max_tool_chain_length: 10,
            blocked_tool_chains: vec![],
        }
    }
}

// ── Models ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelsConfig {
    #[serde(default = "ModelsConfig::default_model")]
    pub default: String,
    #[serde(default = "ModelsConfig::default_kernel")]
    pub kernel: String,
    #[serde(default = "ModelsConfig::default_fallback")]
    pub fallback: String,
    #[serde(default = "ModelsConfig::default_temperature")]
    pub temperature: f32,
}

impl ModelsConfig {
    fn default_model() -> String {
        "claude-sonnet-4".into()
    }
    fn default_kernel() -> String {
        "claude-opus-4".into()
    }
    fn default_fallback() -> String {
        "claude-haiku-4".into()
    }
    fn default_temperature() -> f32 {
        0.7
    }
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            default: "claude-sonnet-4".into(),
            kernel: "claude-opus-4".into(),
            fallback: "claude-haiku-4".into(),
            temperature: 0.7,
        }
    }
}

// ── Observability ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityConfig {
    #[serde(default = "ObservabilityConfig::default_log_level")]
    pub log_level: LogLevel,
    #[serde(default = "ObservabilityConfig::default_log_path")]
    pub log_path: String,
    #[serde(default = "ObservabilityConfig::default_metrics_enabled")]
    pub metrics_enabled: bool,
    #[serde(default = "ObservabilityConfig::default_metrics_path")]
    pub metrics_path: String,
    #[serde(default)]
    pub trace_enabled: bool,
}

impl ObservabilityConfig {
    fn default_log_level() -> LogLevel {
        LogLevel::Info
    }
    fn default_log_path() -> String {
        "/var/log/avix/kernel.log".into()
    }
    fn default_metrics_enabled() -> bool {
        true
    }
    fn default_metrics_path() -> String {
        "/var/log/avix/metrics/".into()
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Info,
            log_path: "/var/log/avix/kernel.log".into(),
            metrics_enabled: true,
            metrics_path: "/var/log/avix/metrics/".into(),
            trace_enabled: false,
        }
    }
}

// ── Secrets ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SecretAlgorithm {
    #[default]
    #[serde(rename = "aes-256-gcm")]
    Aes256Gcm,
    #[serde(rename = "chacha20-poly1305")]
    Chacha20Poly1305,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MasterKeySource {
    Passphrase,
    KeyFile,
    #[default]
    Env,
    KmsAws,
    KmsGcp,
    KmsAzure,
    KmsVault,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MasterKeyConfig {
    pub source: MasterKeySource,
    /// For `source: env` — name of the environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    /// For `source: key-file` — path to the key file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
    /// KDF algorithm when source is `passphrase`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf_algorithm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf_memory_mb: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf_iterations: Option<u32>,
}

impl Default for MasterKeyConfig {
    fn default() -> Self {
        Self {
            source: MasterKeySource::Env,
            env_var: Some("AVIX_MASTER_KEY".into()),
            key_file: None,
            kdf_algorithm: None,
            kdf_memory_mb: None,
            kdf_iterations: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SecretStoreConfig {
    #[serde(default = "SecretStoreConfig::default_path")]
    pub path: String,
    #[serde(default = "SecretStoreConfig::default_provider")]
    pub provider: String,
}

impl SecretStoreConfig {
    fn default_path() -> String {
        "/secrets".into()
    }
    fn default_provider() -> String {
        "local".into()
    }
}

impl Default for SecretStoreConfig {
    fn default() -> Self {
        Self {
            path: "/secrets".into(),
            provider: "local".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretAuditConfig {
    #[serde(default = "SecretAuditConfig::default_enabled")]
    pub enabled: bool,
    #[serde(default = "SecretAuditConfig::default_log_path")]
    pub log_path: String,
    #[serde(default = "SecretAuditConfig::default_log_reads")]
    pub log_reads: bool,
    #[serde(default = "SecretAuditConfig::default_log_writes")]
    pub log_writes: bool,
}

impl SecretAuditConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_log_path() -> String {
        "/var/log/avix/secrets-audit.log".into()
    }
    fn default_log_reads() -> bool {
        true
    }
    fn default_log_writes() -> bool {
        true
    }
}

impl Default for SecretAuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            log_path: "/var/log/avix/secrets-audit.log".into(),
            log_reads: true,
            log_writes: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsConfig {
    #[serde(default)]
    pub algorithm: SecretAlgorithm,
    #[serde(default = "MasterKeyConfig::default")]
    pub master_key: MasterKeyConfig,
    #[serde(default)]
    pub store: SecretStoreConfig,
    #[serde(default)]
    pub audit: SecretAuditConfig,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            algorithm: SecretAlgorithm::Aes256Gcm,
            master_key: MasterKeyConfig::default(),
            store: SecretStoreConfig::default(),
            audit: SecretAuditConfig::default(),
        }
    }
}

// ── KernelSpec / KernelConfig ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KernelSpec {
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub ipc: IpcConfig,
    #[serde(default)]
    pub safety: SafetyConfig,
    #[serde(default)]
    pub models: ModelsConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
    #[serde(default)]
    pub secrets: SecretsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub spec: KernelSpec,
}

impl KernelConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Validate field constraints. Returns `Err` on any violation.
    pub fn validate(&self) -> Result<(), AvixError> {
        let spec = &self.spec;

        if spec.scheduler.tick_ms == 0 {
            return Err(AvixError::ConfigParse(
                "scheduler.tickMs must be > 0".into(),
            ));
        }
        if spec.scheduler.max_concurrent_agents == 0 {
            return Err(AvixError::ConfigParse(
                "scheduler.maxConcurrentAgents must be >= 1".into(),
            ));
        }
        if spec.ipc.max_message_bytes < 4096 {
            return Err(AvixError::ConfigParse(
                "ipc.maxMessageBytes must be >= 4096".into(),
            ));
        }
        if spec.ipc.timeout_ms == 0 {
            return Err(AvixError::ConfigParse("ipc.timeoutMs must be > 0".into()));
        }
        if spec.safety.max_tool_chain_length == 0 {
            return Err(AvixError::ConfigParse(
                "safety.maxToolChainLength must be >= 1".into(),
            ));
        }
        if !(0.0..=2.0).contains(&spec.models.temperature) {
            return Err(AvixError::ConfigParse(
                "models.temperature must be in range [0.0, 2.0]".into(),
            ));
        }
        if spec.models.default.is_empty() {
            return Err(AvixError::ConfigParse(
                "models.default must not be empty".into(),
            ));
        }
        if spec.models.kernel.is_empty() {
            return Err(AvixError::ConfigParse(
                "models.kernel must not be empty".into(),
            ));
        }
        if spec.models.fallback.is_empty() {
            return Err(AvixError::ConfigParse(
                "models.fallback must not be empty".into(),
            ));
        }
        if spec.observability.log_path.is_empty() {
            return Err(AvixError::ConfigParse(
                "observability.logPath must not be empty".into(),
            ));
        }
        if spec.memory.default_context_limit < 1000 {
            return Err(AvixError::ConfigParse(
                "memory.defaultContextLimit must be >= 1000".into(),
            ));
        }

        Ok(())
    }

    /// Returns `true` if changing from `self` to `other` requires a full kernel restart.
    pub fn requires_restart(&self, other: &KernelConfig) -> bool {
        let a = &self.spec;
        let b = &other.spec;
        a.ipc.transport != b.ipc.transport
            || a.ipc.socket_name != b.ipc.socket_name
            || a.ipc.max_message_bytes != b.ipc.max_message_bytes
            || a.models.kernel != b.models.kernel
            || a.secrets.master_key != b.secrets.master_key
            || a.secrets.store != b.secrets.store
    }
}
