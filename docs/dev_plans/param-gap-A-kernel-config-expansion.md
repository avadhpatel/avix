# Param Gap A — KernelConfig Full Schema Expansion

> **Status:** Not started
> **Priority:** High — blocks config reload and correct boot behaviour
> **Affects:** `avix-core/src/config/kernel.rs`, `avix-core/src/cli/config_init.rs`, `avix-core/src/bootstrap/phase2.rs`

---

## Problem

`config/kernel.rs` defines a minimal `KernelSpec` with only `ipc` and `model` fields.
The authoritative spec (`docs/spec/kernel-config.md`) defines six top-level sections:
`scheduler`, `memory`, `ipc`, `safety`, `models`, `observability`, and `secrets`.

None of the missing sections are parsed, validated, or respected at runtime. The
`config_init` template for `kernel.yaml` was written before the full spec existed and
does not emit all required sections. There is no reload mechanism — `avix reload` is
completely unimplemented.

---

## What Needs to Be Built

### 1. Expand `KernelSpec` and sub-structs (`config/kernel.rs`)

Replace the current minimal struct with the full spec:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelSpec {
    pub scheduler: SchedulerConfig,
    pub memory: MemoryConfig,
    pub ipc: IpcConfig,
    pub safety: SafetyConfig,
    pub models: ModelsConfig,
    pub observability: ObservabilityConfig,
    pub secrets: SecretsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerConfig {
    pub algorithm: SchedulerAlgorithm,  // enum: PriorityDeadline | RoundRobin | Fifo
    pub tick_ms: u32,
    pub preemption: bool,
    pub max_concurrent_agents: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConfig {
    pub default_context_limit: u32,    // tokens
    pub eviction_policy: EvictionPolicy,  // enum: Lru | LruSalience | Manual
    pub max_episodic_retention_days: u32,
    pub shared_memory_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SafetyConfig {
    pub policy_engine: PolicyEngineMode,  // enum: Enabled | Disabled
    pub hil_on_escalation: bool,
    pub max_tool_chain_length: u32,
    pub blocked_tool_chains: Vec<BlockedToolChain>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockedToolChain {
    pub pattern: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsConfig {
    pub default: String,
    pub kernel: String,
    pub fallback: String,
    pub temperature: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityConfig {
    pub log_level: LogLevel,  // enum: Debug | Info | Warn | Error
    pub log_path: String,
    pub metrics_enabled: bool,
    pub metrics_path: String,
    pub trace_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsConfig {
    pub algorithm: SecretAlgorithm,  // enum: Aes256Gcm | Chacha20Poly1305
    pub master_key: MasterKeyConfig,
    pub store: SecretStoreConfig,
    pub audit: SecretAuditConfig,
}

// ... MasterKeyConfig, SecretStoreConfig, SecretAuditConfig similarly
```

All fields must have `Default` implementations matching the spec's defaults table:

| Field | Default |
|-------|---------|
| `scheduler.algorithm` | `PriorityDeadline` |
| `scheduler.tick_ms` | `100` |
| `scheduler.preemption` | `true` |
| `scheduler.max_concurrent_agents` | `50` |
| `memory.default_context_limit` | `200000` |
| `memory.eviction_policy` | `LruSalience` |
| `ipc.max_message_bytes` | `65536` |
| `ipc.timeout_ms` | `5000` |
| `safety.policy_engine` | `Enabled` |
| `safety.hil_on_escalation` | `true` |
| `safety.max_tool_chain_length` | `10` |
| `models.temperature` | `0.7` |
| `observability.log_level` | `Info` |
| `observability.metrics_enabled` | `true` |
| `observability.trace_enabled` | `false` |

### 2. Add `KernelConfig::validate()` method

```rust
impl KernelConfig {
    pub fn validate(&self) -> Result<(), AvixError> {
        // scheduler.tick_ms must be > 0
        // scheduler.max_concurrent_agents must be 1..=10000
        // memory.default_context_limit must be >= 1000
        // ipc.max_message_bytes must be >= 4096 and <= 16MB
        // ipc.timeout_ms must be > 0
        // safety.max_tool_chain_length must be 1..=200
        // models.temperature must be 0.0..=2.0
        // models.default, .kernel, .fallback must be non-empty strings
        // observability.log_path must be non-empty
        // secrets.master_key.source must be valid enum variant
    }
}
```

### 3. Reload-capable sections

Add a marker trait or method that indicates which sections support hot reload:

```rust
impl KernelConfig {
    /// Returns true if changing from `self` to `other` requires a full restart.
    pub fn requires_restart(&self, other: &KernelConfig) -> bool {
        self.spec.ipc != other.spec.ipc
            || self.spec.models.kernel != other.spec.models.kernel
            || self.spec.secrets.master_key != other.spec.secrets.master_key
            || self.spec.secrets.store != other.spec.secrets.store
    }
}
```

Bootstrap Phase 2 reads `kernel.yaml` and calls `validate()` — boot must abort on
validation failure (per Architecture Invariant #1).

### 4. Update `config_init` kernel.yaml template

`cli/config_init.rs` currently emits a minimal `kernel.yaml`. Expand it to emit all
spec-required sections with defaults. The template should produce a valid `KernelConfig`
that passes `validate()` immediately after `config init`.

```yaml
apiVersion: avix/v1
kind: KernelConfig
metadata:
  lastUpdated: <ISO8601>

spec:
  scheduler:
    algorithm: priority_deadline
    tickMs: 100
    preemption: true
    maxConcurrentAgents: 50

  memory:
    defaultContextLimit: 200000
    evictionPolicy: lru_salience
    maxEpisodicRetentionDays: 30
    sharedMemoryPath: /shared/

  ipc:
    transport: unix-socket
    socketPath: /var/run/avix/kernel.sock
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
```

---

## TDD Test Plan

All tests go in `crates/avix-core/tests/config.rs` (add to the existing suite).

```rust
// T-A-01: Full KernelConfig parses correctly from spec example YAML
#[test]
fn kernel_config_full_parse() {
    let yaml = include_str!("fixtures/kernel_config_full.yaml");
    let cfg: KernelConfig = KernelConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.spec.scheduler.algorithm, SchedulerAlgorithm::PriorityDeadline);
    assert_eq!(cfg.spec.scheduler.tick_ms, 100);
    assert_eq!(cfg.spec.memory.default_context_limit, 200000);
    assert_eq!(cfg.spec.safety.max_tool_chain_length, 10);
    assert!(!cfg.spec.observability.trace_enabled);
    assert_eq!(cfg.spec.models.temperature, 0.7);
}

// T-A-02: Default values applied when fields omitted
#[test]
fn kernel_config_defaults_applied() {
    let minimal = "apiVersion: avix/v1\nkind: KernelConfig\nspec:\n  ipc:\n    transport: unix-socket\n";
    let cfg = KernelConfig::from_str(minimal).unwrap();
    assert_eq!(cfg.spec.scheduler.tick_ms, 100);
    assert_eq!(cfg.spec.scheduler.max_concurrent_agents, 50);
    assert!(cfg.spec.safety.hil_on_escalation);
}

// T-A-03: validate() rejects tick_ms == 0
#[test]
fn kernel_config_validate_rejects_zero_tick() {
    let mut cfg = KernelConfig::default();
    cfg.spec.scheduler.tick_ms = 0;
    assert!(cfg.validate().is_err());
}

// T-A-04: validate() rejects temperature > 2.0
#[test]
fn kernel_config_validate_rejects_bad_temperature() {
    let mut cfg = KernelConfig::default();
    cfg.spec.models.temperature = 3.0;
    assert!(cfg.validate().is_err());
}

// T-A-05: requires_restart is true when ipc section changes
#[test]
fn kernel_config_restart_required_for_ipc_change() {
    let a = KernelConfig::default();
    let mut b = a.clone();
    b.spec.ipc.timeout_ms = 9999;
    assert!(a.requires_restart(&b));
}

// T-A-06: requires_restart is false for observability change
#[test]
fn kernel_config_no_restart_for_observability_change() {
    let a = KernelConfig::default();
    let mut b = a.clone();
    b.spec.observability.log_level = LogLevel::Debug;
    assert!(!a.requires_restart(&b));
}

// T-A-07: config_init writes valid kernel.yaml that passes validate()
#[tokio::test]
async fn config_init_kernel_yaml_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    run_config_init(ConfigInitParams { root: dir.path().to_path_buf(), .. }).await.unwrap();
    let raw = std::fs::read_to_string(dir.path().join("etc/kernel.yaml")).unwrap();
    let cfg = KernelConfig::from_str(&raw).unwrap();
    cfg.validate().unwrap();
}

// T-A-08: config_init kernel.yaml includes all spec sections
#[tokio::test]
async fn config_init_kernel_yaml_has_all_sections() {
    let dir = tempfile::tempdir().unwrap();
    run_config_init(ConfigInitParams { root: dir.path().to_path_buf(), .. }).await.unwrap();
    let raw = std::fs::read_to_string(dir.path().join("etc/kernel.yaml")).unwrap();
    assert!(raw.contains("scheduler:"));
    assert!(raw.contains("memory:"));
    assert!(raw.contains("safety:"));
    assert!(raw.contains("models:"));
    assert!(raw.contains("observability:"));
    assert!(raw.contains("secrets:"));
}
```

---

## Implementation Notes

- Keep `IpcConfig` as-is (already tested elsewhere); extend `KernelSpec` around it.
- Use `#[serde(default)]` on each sub-struct field to apply spec-table defaults without
  requiring every field to be present in the YAML.
- Enum variants (`SchedulerAlgorithm`, `EvictionPolicy`, etc.) must implement
  `Serialize`/`Deserialize` with `#[serde(rename_all = "snake_case")]`.
- `SecretsConfig` does not need to implement `Default` for `master_key.source` — it must
  be present in the file. Validate that `master_key.source` is not missing.
- Do **not** implement actual reload logic here (that belongs in Phase 2 bootstrap or a
  separate reload command) — just provide `requires_restart()` as a helper.

---

## Success Criteria

- [ ] `KernelConfig::from_str` parses all spec fields without errors
- [ ] `KernelConfig::validate()` returns `Ok` for all spec-example inputs
- [ ] `KernelConfig::validate()` returns `Err` for each known invalid input (tests T-A-03, T-A-04)
- [ ] `requires_restart()` correctly classifies all sections per the spec table
- [ ] `config_init` writes a `kernel.yaml` that round-trips through parse+validate
- [ ] All T-A-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
