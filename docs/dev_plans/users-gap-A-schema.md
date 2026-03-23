# Users Gap A — Full Schema Alignment

> **Status:** Not started
> **Priority:** High — users.yaml is parsed by config_init, `avix resolve`, and will be used by auth at spawn
> **Depends on:** None (self-contained to `crates/avix-core/src/config/users.rs`)
> **Affects:** `avix-core/src/config/users.rs`, `avix-core/src/cli/config_init.rs`, `avix-core/tests/config.rs`, `avix-core/src/cli/resolve.rs`

---

## Problem

The current `UsersConfig` / `User` structs do not match the spec in four significant ways:

1. **Structure mismatch.** The spec uses `spec.users[]` (with a `spec:` wrapper and `metadata:`), but `UsersConfig` currently parses a flat `users:` key. The `config_init` template *correctly* writes `spec.users:`, which means `UsersConfig::from_str()` silently ignores the `spec` wrapper and would deserialise an empty user list.

2. **Wrong field names.** Spec says `username:`, `workspace:` / `home:`, `shell:`; implementation has `name:` only.

3. **Incomplete `UserQuota`.** Spec defines `tokens`, `agents`, `sessions` (each either an integer or the string `"unlimited"`). Current implementation has `tokens: Option<u64>` and `requestsPerDay: Option<u64>` — neither matches the spec.

4. **Missing fields.** `crews: Vec<String>`, `workspace`/`home`, `shell`, and `tools` (for root) are absent from the struct.

---

## What Needs to Be Built

### `QuotaValue` — represent "unlimited" or a number

```rust
/// A quota field value — either a concrete limit or unlimited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuotaValue {
    /// Concrete limit (positive integer).
    Count(u64),
    /// The string "unlimited".
    Unlimited(String),
}

impl QuotaValue {
    pub fn is_unlimited(&self) -> bool {
        matches!(self, QuotaValue::Unlimited(_))
    }
    pub fn count(&self) -> Option<u64> {
        match self {
            QuotaValue::Count(n) => Some(*n),
            _ => None,
        }
    }
}
```

Validation: if the `Unlimited` variant is present, the inner String must equal `"unlimited"`.

### `UserQuota` — updated fields

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuota {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<QuotaValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<QuotaValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<QuotaValue>,
}
```

### `User` — full spec-compliant struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub username: String,
    pub uid: u32,
    /// Primary workspace path for regular users: `/users/<username>/workspace`.
    /// Present for non-root users.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Home path for root: `/root`. Present only when workspace is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// `/bin/sh` for interactive users; `nologin` for service accounts.
    #[serde(default = "User::default_shell")]
    pub shell: String,
    /// Crew names this user belongs to.
    #[serde(default)]
    pub crews: Vec<String>,
    /// Root only: `tools: [all]` grants access to every tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Tools granted on top of crew's `allowedTools`.
    #[serde(default)]
    pub additional_tools: Vec<String>,
    /// Tools explicitly blocked even if a crew would allow them.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<UserQuota>,
}

impl User {
    fn default_shell() -> String {
        "/bin/sh".into()
    }

    pub fn is_service_account(&self) -> bool {
        self.shell == "nologin"
    }

    pub fn workspace_path(&self) -> String {
        self.workspace.clone()
            .or_else(|| self.home.clone())
            .unwrap_or_else(|| format!("/users/{}/workspace", self.username))
    }
}
```

### `UsersMetadata` and `UsersSpec` wrappers

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsersMetadata {
    #[serde(default)]
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsersSpec {
    pub users: Vec<User>,
}

pub struct UsersConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: UsersMetadata,
    pub spec: UsersSpec,
}
```

Accessor on `UsersConfig`:
```rust
pub fn users(&self) -> &[User] { &self.spec.users }
pub fn find_user(&self, username: &str) -> Option<&User> {
    self.spec.users.iter().find(|u| u.username == username)
}
```

### Validation rules (updated)

```rust
fn validate(&self) -> Result<(), AvixError> {
    let mut seen_uids = HashSet::new();
    for user in &self.spec.users {
        // Duplicate UID check
        if !seen_uids.insert(user.uid) {
            return Err(AvixError::ConfigParse(format!("duplicate uid: {}", user.uid)));
        }
        // Reserved UID range check (warn for 1-999 unless it's uid 0 = root)
        if user.uid > 0 && user.uid < 1000 {
            return Err(AvixError::ConfigParse(format!(
                "uid {} is in reserved range 1-999 (user '{}')",
                user.uid, user.username
            )));
        }
        // workspace XOR home: at most one should be set
        if user.workspace.is_some() && user.home.is_some() {
            return Err(AvixError::ConfigParse(format!(
                "user '{}' must have workspace OR home, not both",
                user.username
            )));
        }
        // Validate QuotaValue "unlimited" strings
        if let Some(q) = &user.quota {
            for (label, val) in [("tokens", &q.tokens), ("agents", &q.agents), ("sessions", &q.sessions)] {
                if let Some(QuotaValue::Unlimited(s)) = val {
                    if s != "unlimited" {
                        return Err(AvixError::ConfigParse(format!(
                            "user '{}' quota.{label}: invalid string '{s}' (expected 'unlimited')",
                            user.username
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
```

### Updated `config_init` template (`USERS_YAML_TEMPLATE`)

```yaml
apiVersion: avix/v1
kind: Users
metadata:
  lastUpdated: "{now}"

spec:
  users:
    - username: "{identity}"
      uid: 1001
      workspace: /users/{identity}/workspace
      shell: /bin/sh
      crews: []
      additionalTools: []
      deniedTools: []
      quota:
        tokens: 500000
        agents: 5
        sessions: 4
```

### Updated `resolve.rs` `UserEntry`

`cli/resolve.rs` has a private `UserEntry` struct for parsing users.yaml. Update it to use the proper types:

```rust
// Replace private UserEntry with direct use of UsersConfig
use crate::config::users::UsersConfig;

// In run_resolve():
let users_config = UsersConfig::from_str(&users_yaml)?;
let user = users_config.find_user(&params.username)
    .ok_or_else(|| AvixError::ConfigParse(format!("user '{}' not found", params.username)))?;
let mut crews = user.crews.clone();
```

---

## TDD Test Plan

File: `crates/avix-core/tests/config.rs` (replace existing UsersConfig tests)

```rust
fn users_yaml_full() -> &'static str {
    r#"
apiVersion: avix/v1
kind: Users
metadata:
  lastUpdated: "2026-03-20T00:00:00Z"
spec:
  users:
    - username: root
      uid: 0
      home: /root
      shell: /bin/sh
      crews: [all, kernel]
      tools: [all]
      quota:
        tokens: unlimited
        agents: unlimited
        sessions: unlimited

    - username: alice
      uid: 1001
      workspace: /users/alice/workspace
      shell: /bin/sh
      crews: [researchers, writers]
      additionalTools:
        - python
      deniedTools: []
      quota:
        tokens: 500000
        agents: 5
        sessions: 4

    - username: svc-pipeline
      uid: 2001
      workspace: /services/svc-pipeline/workspace
      shell: nologin
      crews: [automation]
      quota:
        tokens: 1000000
        agents: 10
        sessions: 1
"#
}

// T-UA-01: Full spec-compliant YAML parses
#[test]
fn users_config_full_parse() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    assert_eq!(cfg.users().len(), 3);
}

// T-UA-02: spec wrapper is required (flat users: fails)
#[test]
fn users_config_flat_users_fails() {
    let yaml = "apiVersion: avix/v1\nkind: Users\nusers:\n  - username: alice\n    uid: 1001\n";
    assert!(UsersConfig::from_str(yaml).is_err());
}

// T-UA-03: username field (not name)
#[test]
fn users_config_username_field() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    assert!(cfg.find_user("alice").is_some());
    assert!(cfg.find_user("nonexistent").is_none());
}

// T-UA-04: workspace vs home
#[test]
fn users_config_workspace_and_home() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    let root = cfg.find_user("root").unwrap();
    assert_eq!(root.home.as_deref(), Some("/root"));
    assert!(root.workspace.is_none());
    let alice = cfg.find_user("alice").unwrap();
    assert_eq!(alice.workspace.as_deref(), Some("/users/alice/workspace"));
    assert!(alice.home.is_none());
}

// T-UA-05: shell field; service accounts
#[test]
fn users_config_shell_and_service_account() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    let svc = cfg.find_user("svc-pipeline").unwrap();
    assert_eq!(svc.shell, "nologin");
    assert!(svc.is_service_account());
    let alice = cfg.find_user("alice").unwrap();
    assert!(!alice.is_service_account());
}

// T-UA-06: crews membership list
#[test]
fn users_config_crews() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    let alice = cfg.find_user("alice").unwrap();
    assert!(alice.crews.contains(&"researchers".to_string()));
    assert!(alice.crews.contains(&"writers".to_string()));
}

// T-UA-07: quota with unlimited values
#[test]
fn users_config_quota_unlimited() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    let root = cfg.find_user("root").unwrap();
    let quota = root.quota.as_ref().unwrap();
    assert!(quota.tokens.as_ref().unwrap().is_unlimited());
    assert!(quota.agents.as_ref().unwrap().is_unlimited());
}

// T-UA-08: quota with numeric values
#[test]
fn users_config_quota_numeric() {
    let cfg = UsersConfig::from_str(users_yaml_full()).unwrap();
    let alice = cfg.find_user("alice").unwrap();
    let quota = alice.quota.as_ref().unwrap();
    assert_eq!(quota.tokens.as_ref().unwrap().count(), Some(500_000));
    assert_eq!(quota.agents.as_ref().unwrap().count(), Some(5));
    assert_eq!(quota.sessions.as_ref().unwrap().count(), Some(4));
}

// T-UA-09: duplicate UIDs rejected
#[test]
fn users_config_rejects_duplicate_uids() {
    let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
        - username: alice\n    uid: 1001\n  - username: bob\n    uid: 1001\n";
    assert!(UsersConfig::from_str(yaml).is_err());
}

// T-UA-10: reserved UID range 1-999 rejected
#[test]
fn users_config_rejects_reserved_uid_range() {
    let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
        - username: svc\n    uid: 500\n";
    assert!(UsersConfig::from_str(yaml).is_err());
}

// T-UA-11: workspace XOR home; both set is an error
#[test]
fn users_config_rejects_both_workspace_and_home() {
    let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
        - username: alice\n    uid: 1001\n    workspace: /users/alice/workspace\n    home: /home/alice\n";
    assert!(UsersConfig::from_str(yaml).is_err());
}

// T-UA-12: config_init template is spec-compliant
#[test]
fn users_config_init_template_parses() {
    // Generate template then parse it
    use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: dir.path().to_path_buf(),
        identity_name: "alice".into(),
        credential_type: "api_key".into(),
        role: "admin".into(),
        master_key_source: "env".into(),
        mode: "cli".into(),
    }).unwrap();
    let yaml = std::fs::read_to_string(dir.path().join("etc/users.yaml")).unwrap();
    let cfg = UsersConfig::from_str(&yaml).unwrap();
    assert!(cfg.find_user("alice").is_some());
}
```

---

## Implementation Notes

- `QuotaValue::Unlimited(String)` with `#[serde(untagged)]` means serde tries `Count(u64)` first; if the YAML value is the string `"unlimited"` it falls through to `Unlimited(String)`. Validation then checks the inner string equals `"unlimited"`.
- Remove `requestsPerDay` from `UserQuota` — it is not in the spec. The existing test that checks it must be replaced.
- The existing `resolve.rs` private `UserEntry` struct should be deleted and replaced by direct use of `UsersConfig`.
- Existing tests that use `u.name` or flat `users:` YAML must be replaced by the new tests above.
- The `config_init.rs` test `config_init_users_yaml_has_all_fields` should be updated to check for `username`, `workspace`, `crews`, and `quota` fields.

---

## Success Criteria

- [ ] `UsersConfig::from_str()` parses the spec-example YAML (T-UA-01)
- [ ] Flat `users:` at root level fails (T-UA-02)
- [ ] `username` field used everywhere (T-UA-03)
- [ ] `workspace` / `home` correctly parsed and exclusive (T-UA-04)
- [ ] `shell` and `is_service_account()` work (T-UA-05)
- [ ] `crews` field on User (T-UA-06)
- [ ] `QuotaValue::Unlimited` parsed from `"unlimited"` (T-UA-07)
- [ ] `QuotaValue::Count` parsed from integer (T-UA-08)
- [ ] Duplicate UID validation (T-UA-09)
- [ ] Reserved UID range rejected (T-UA-10)
- [ ] `workspace` XOR `home` enforced (T-UA-11)
- [ ] `config_init` template is parseable (T-UA-12)
- [ ] `cargo clippy --workspace -- -D warnings` passes
