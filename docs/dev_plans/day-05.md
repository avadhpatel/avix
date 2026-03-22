# Day 5 — Config Parsing & Validation

> **Goal:** Parse and validate every YAML config file the system depends on: `AuthConfig`, `KernelConfig`, `UsersConfig`, `CrewsConfig`, and the new `LlmConfig`. All parsing is strict — unknown fields are rejected. Tests verify both happy paths and every important error case.

---

## Pre-flight: Verify Day 4

```bash
cargo test --workspace
# Expected: all Day 4 tests pass (10+ signal bus tests)

# Confirm SignalBus exists
grep -r "pub struct SignalBus" crates/avix-core/src/
grep -r "pub enum SignalKind"  crates/avix-core/src/

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings
```

All checks must pass before writing new code.

---

## Step 1 — Extend the Module Tree

Add to `crates/avix-core/src/lib.rs`:

```rust
pub mod error;
pub mod types;
pub mod process;
pub mod signal;
pub mod config;   // NEW
```

Create:

```
crates/avix-core/src/config/
├── mod.rs
├── auth.rs          ← AuthConfig (auth.conf)
├── kernel.rs        ← KernelConfig (kernel.yaml)
├── users.rs         ← UsersConfig (users.yaml)
├── crews.rs         ← CrewsConfig (crews.yaml)
└── llm.rs           ← LlmConfig (llm.yaml)
```

**`src/config/mod.rs`**

```rust
pub mod auth;
pub mod crews;
pub mod kernel;
pub mod llm;
pub mod users;

pub use auth::{AuthConfig, AuthIdentity, CredentialType};
pub use crews::{CrewsConfig, Crew};
pub use kernel::{IpcTransportKind, KernelConfig};
pub use llm::{LlmConfig, ProviderAuth, ProviderConfig};
pub use users::{User, UsersConfig};
```

---

## Step 2 — Write Tests First

Create `crates/avix-core/tests/config.rs`:

```rust
use avix_core::config::*;
use avix_core::types::Modality;

// ── AuthConfig ────────────────────────────────────────────────────────────────

#[test]
fn auth_config_parses_api_key_identity() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
  require_tls: true
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: api_key
      key_hash: "hmac-sha256:abc123"
"#;
    let cfg = AuthConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.identities.len(), 1);
    assert_eq!(cfg.identities[0].name, "alice");
    assert_eq!(cfg.identities[0].uid, 1001);
    assert!(matches!(cfg.identities[0].credential, CredentialType::ApiKey { .. }));
}

#[test]
fn auth_config_parses_password_identity() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 4h
identities:
  - name: bob
    uid: 1002
    role: user
    credential:
      type: password
      password_hash: "argon2:somehash"
"#;
    let cfg = AuthConfig::from_str(yaml).unwrap();
    assert!(matches!(cfg.identities[0].credential, CredentialType::Password { .. }));
}

#[test]
fn auth_config_rejects_credential_type_none() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
identities:
  - name: alice
    uid: 1001
    role: admin
    credential:
      type: none
"#;
    let result = AuthConfig::from_str(yaml);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("none") || msg.contains("credential"));
}

#[test]
fn auth_config_requires_at_least_one_admin() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
identities:
  - name: bob
    uid: 1002
    role: user
    credential:
      type: api_key
      key_hash: "hmac-sha256:abc"
"#;
    let result = AuthConfig::from_str(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("admin"));
}

#[test]
fn auth_config_empty_identities_is_error() {
    let yaml = r#"
apiVersion: avix/v1
kind: AuthConfig
policy:
  session_ttl: 8h
identities: []
"#;
    assert!(AuthConfig::from_str(yaml).is_err());
}

// ── KernelConfig ──────────────────────────────────────────────────────────────

#[test]
fn kernel_config_parses_local_ipc_transport() {
    let yaml = r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  ipc:
    transport: local-ipc
    socket_name: kernel
  model:
    tier: premium
    provider: anthropic
"#;
    let cfg = KernelConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.spec.ipc.transport, IpcTransportKind::LocalIpc);
    assert_eq!(cfg.spec.ipc.socket_name, "kernel");
}

#[test]
fn kernel_config_rejects_unknown_transport() {
    let yaml = r#"
apiVersion: avix/v1
kind: KernelConfig
spec:
  ipc:
    transport: grpc
    socket_name: kernel
"#;
    assert!(KernelConfig::from_str(yaml).is_err());
}

// ── UsersConfig ───────────────────────────────────────────────────────────────

#[test]
fn users_config_parses_user_with_tools() {
    let yaml = r#"
apiVersion: avix/v1
kind: UsersConfig
users:
  - name: alice
    uid: 1001
    role: admin
    additionalTools: ["web_search", "send_email"]
    deniedTools: []
    quota:
      tokens: 1000000
      requestsPerDay: 500
"#;
    let cfg = UsersConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.users[0].name, "alice");
    assert!(cfg.users[0].additional_tools.contains(&"web_search".to_string()));
}

#[test]
fn users_config_rejects_duplicate_uid() {
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

// ── CrewsConfig ───────────────────────────────────────────────────────────────

#[test]
fn crews_config_parses_crew() {
    let yaml = r#"
apiVersion: avix/v1
kind: CrewsConfig
crews:
  - cid: research-crew
    members: [alice, bob]
    allowedTools: ["web_search", "fs/read"]
    deniedTools: ["send_email"]
"#;
    let cfg = CrewsConfig::from_str(yaml).unwrap();
    assert_eq!(cfg.crews[0].cid, "research-crew");
    assert!(cfg.crews[0].allowed_tools.contains(&"web_search".to_string()));
    assert!(cfg.crews[0].denied_tools.contains(&"send_email".to_string()));
}

// ── LlmConfig ─────────────────────────────────────────────────────────────────

const VALID_LLM_YAML: &str = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: openai
    speech: elevenlabs
    transcription: openai
    embedding: openai
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: llm-anthropic-key
        header: x-api-key
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: oauth2
        secretName: llm-openai-oauth
        tokenUrl: https://auth.openai.com/oauth/token
        clientId: avix-client
        clientSecretName: llm-openai-client-secret
        scopes: [model.read, completions.write]
        refreshBeforeExpiryMin: 5
    - name: elevenlabs
      baseUrl: https://api.elevenlabs.io
      modalities: [speech]
      auth:
        type: api_key
        secretName: llm-elevenlabs-key
        header: xi-api-key
    - name: ollama
      baseUrl: http://localhost:11434
      modalities: [text, embedding]
      auth:
        type: none
"#;

#[test]
fn llm_config_parses_default_providers() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    assert_eq!(cfg.spec.default_providers.text,          "anthropic");
    assert_eq!(cfg.spec.default_providers.image,         "openai");
    assert_eq!(cfg.spec.default_providers.speech,        "elevenlabs");
    assert_eq!(cfg.spec.default_providers.transcription, "openai");
    assert_eq!(cfg.spec.default_providers.embedding,     "openai");
}

#[test]
fn llm_config_parses_api_key_auth() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    let anthropic = cfg.spec.providers.iter().find(|p| p.name == "anthropic").unwrap();
    assert!(matches!(anthropic.auth, ProviderAuth::ApiKey { .. }));
}

#[test]
fn llm_config_parses_oauth2_auth() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    let openai = cfg.spec.providers.iter().find(|p| p.name == "openai").unwrap();
    assert!(matches!(openai.auth, ProviderAuth::Oauth2 { .. }));
}

#[test]
fn llm_config_parses_none_auth_for_ollama() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    let ollama = cfg.spec.providers.iter().find(|p| p.name == "ollama").unwrap();
    assert!(matches!(ollama.auth, ProviderAuth::None));
}

#[test]
fn llm_config_rejects_default_provider_wrong_modality() {
    // defaultProviders.image: anthropic, but anthropic only supports text
    let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: anthropic
    image: anthropic
    speech: anthropic
    transcription: anthropic
    embedding: anthropic
  providers:
    - name: anthropic
      baseUrl: https://api.anthropic.com
      modalities: [text]
      auth:
        type: api_key
        secretName: k
        header: x-api-key
"#;
    let result = LlmConfig::from_str(yaml);
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("modality") || msg.contains("support"));
}

#[test]
fn llm_config_rejects_unknown_default_provider() {
    let yaml = r#"
apiVersion: avix/v1
kind: LlmConfig
spec:
  defaultProviders:
    text: does-not-exist
    image: openai
    speech: openai
    transcription: openai
    embedding: openai
  providers:
    - name: openai
      baseUrl: https://api.openai.com
      modalities: [text, image, speech, transcription, embedding]
      auth:
        type: none
"#;
    assert!(LlmConfig::from_str(yaml).is_err());
}

#[test]
fn llm_config_provider_modalities_parse_correctly() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    let openai = cfg.spec.providers.iter().find(|p| p.name == "openai").unwrap();
    assert!(openai.modalities.contains(&Modality::Text));
    assert!(openai.modalities.contains(&Modality::Image));
    assert!(openai.modalities.contains(&Modality::Speech));
    assert!(openai.modalities.contains(&Modality::Transcription));
    assert!(openai.modalities.contains(&Modality::Embedding));
}

#[test]
fn llm_config_lookup_provider_by_modality() {
    let cfg = LlmConfig::from_str(VALID_LLM_YAML).unwrap();
    let provider = cfg.default_provider_for(Modality::Image).unwrap();
    assert_eq!(provider.name, "openai");
}
```

---

## Step 3 — Implement Config Types

**`src/config/auth.rs`** (key excerpt — full implementation):

```rust
use serde::{Deserialize, Serialize};
use crate::error::AvixError;
use crate::types::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialType {
    ApiKey    { key_hash: String, #[serde(default)] header: Option<String> },
    Password  { password_hash: String },
    // NOTE: `none` is intentionally NOT a variant — it must never exist.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthIdentity {
    pub name:       String,
    pub uid:        u32,
    pub role:       Role,
    pub credential: CredentialType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthPolicy {
    pub session_ttl: String,
    #[serde(default)]
    pub require_tls: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    #[serde(rename = "apiVersion")] pub api_version: String,
    pub kind:        String,
    pub policy:      AuthPolicy,
    pub identities:  Vec<AuthIdentity>,
}

impl AuthConfig {
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self = serde_yaml::from_str(s)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), AvixError> {
        if self.identities.is_empty() {
            return Err(AvixError::ConfigParse("identities must not be empty".into()));
        }
        let has_admin = self.identities.iter().any(|i| i.role == Role::Admin);
        if !has_admin {
            return Err(AvixError::ConfigParse(
                "at least one identity must have role: admin".into()
            ));
        }
        Ok(())
    }
}
```

**`src/config/llm.rs`** (key excerpt):

```rust
use serde::{Deserialize, Serialize};
use crate::error::AvixError;
use crate::types::Modality;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderAuth {
    ApiKey { #[serde(rename = "secretName")] secret_name: String, header: String },
    Oauth2 {
        #[serde(rename = "secretName")] secret_name: String,
        #[serde(rename = "tokenUrl")]   token_url: String,
        #[serde(rename = "clientId")]   client_id: String,
        #[serde(rename = "clientSecretName")] client_secret_name: String,
        scopes: Vec<String>,
        #[serde(rename = "refreshBeforeExpiryMin")] refresh_before_expiry_min: u32,
    },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name:       String,
    #[serde(rename = "baseUrl")] pub base_url: String,
    pub modalities: Vec<Modality>,
    pub auth:       ProviderAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultProviders {
    pub text:          String,
    pub image:         String,
    pub speech:        String,
    pub transcription: String,
    pub embedding:     String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSpec {
    #[serde(rename = "defaultProviders")] pub default_providers: DefaultProviders,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(rename = "apiVersion")] pub api_version: String,
    pub kind: String,
    pub spec: LlmSpec,
}

impl LlmConfig {
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self = serde_yaml::from_str(s)
            .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), AvixError> {
        // Validate each defaultProvider references a known provider that supports the modality
        let check = |modality: Modality, provider_name: &str| -> Result<(), AvixError> {
            let provider = self.spec.providers.iter().find(|p| p.name == provider_name)
                .ok_or_else(|| AvixError::ConfigParse(
                    format!("defaultProvider for {}: '{}' not found in providers list",
                        modality.as_str(), provider_name)
                ))?;
            if !provider.modalities.contains(&modality) {
                return Err(AvixError::ConfigParse(format!(
                    "defaultProvider for {} is '{}' but that provider does not support that modality",
                    modality.as_str(), provider_name
                )));
            }
            Ok(())
        };

        let dp = &self.spec.default_providers;
        check(Modality::Text,          &dp.text)?;
        check(Modality::Image,         &dp.image)?;
        check(Modality::Speech,        &dp.speech)?;
        check(Modality::Transcription, &dp.transcription)?;
        check(Modality::Embedding,     &dp.embedding)?;
        Ok(())
    }

    pub fn default_provider_for(&self, modality: Modality) -> Option<&ProviderConfig> {
        let name = match modality {
            Modality::Text          => &self.spec.default_providers.text,
            Modality::Image         => &self.spec.default_providers.image,
            Modality::Speech        => &self.spec.default_providers.speech,
            Modality::Transcription => &self.spec.default_providers.transcription,
            Modality::Embedding     => &self.spec.default_providers.embedding,
        };
        self.spec.providers.iter().find(|p| &p.name == name)
    }
}
```

Implement `KernelConfig`, `UsersConfig`, and `CrewsConfig` similarly — each with a `from_str` that deserialises from YAML and calls a `validate()` method.

---

## Step 4 — Verify

```bash
cargo test --workspace
# Expected: all Day 5 config tests pass (30+ new tests)

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings

cargo fmt --check
# Expected: exit 0

# Spot-check coverage
cargo tarpaulin --workspace --out Stdout 2>/dev/null | grep "Coverage"
# Expected: avix-core coverage climbing (target ≥ 85% by end of week)
```

---

## Commit

```bash
git add -A
git commit -m "day-05: config parsing — AuthConfig, KernelConfig, UsersConfig, CrewsConfig, LlmConfig"
```

---

## Success Criteria

- [ ] 30+ config parsing tests pass
- [ ] `AuthConfig` rejects `credential.type: none`
- [ ] `AuthConfig` rejects configs with no admin identity
- [ ] `AuthConfig` rejects empty identities list
- [ ] `KernelConfig` rejects unknown transport values
- [ ] `LlmConfig` parses all three auth types (api_key, oauth2, none)
- [ ] `LlmConfig` validates `defaultProviders` against declared provider modalities
- [ ] `LlmConfig.default_provider_for(modality)` returns correct provider
- [ ] Duplicate UID in `UsersConfig` is rejected
- [ ] 0 clippy warnings
