use crate::error::AvixError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ── QuotaValue ────────────────────────────────────────────────────────────────

/// A quota limit — either a concrete count or the string `"unlimited"`.
///
/// Serialises as a plain integer or the string `"unlimited"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QuotaValue {
    /// Concrete positive-integer limit.
    Count(u64),
    /// The literal string `"unlimited"` (any other string is a validation error).
    Unlimited(String),
}

impl QuotaValue {
    pub fn is_unlimited(&self) -> bool {
        matches!(self, QuotaValue::Unlimited(_))
    }

    pub fn count(&self) -> Option<u64> {
        match self {
            QuotaValue::Count(n) => Some(*n),
            QuotaValue::Unlimited(_) => None,
        }
    }
}

// ── UserQuota ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuota {
    /// Rolling 24-hour token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<QuotaValue>,
    /// Maximum concurrently running agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<QuotaValue>,
    /// Maximum concurrent interactive sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<QuotaValue>,
}

// ── User ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub username: String,
    pub uid: u32,
    /// Workspace path for regular users, e.g. `/users/<username>/workspace`.
    /// Mutually exclusive with `home`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Home path for root, e.g. `/root`. Mutually exclusive with `workspace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// `/bin/sh` for interactive users; `nologin` for service accounts.
    #[serde(default = "User::default_shell")]
    pub shell: String,
    /// Crew names this user belongs to.
    #[serde(default)]
    pub crews: Vec<String>,
    /// Root only: `[all]` grants access to every tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Tools granted on top of the crew's `allowedTools`.
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

    /// Returns the canonical VFS path for this user's workspace.
    pub fn workspace_path(&self) -> String {
        self.workspace
            .clone()
            .or_else(|| self.home.clone())
            .unwrap_or_else(|| format!("/users/{}/workspace", self.username))
    }
}

// ── UsersConfig envelope ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsersMetadata {
    #[serde(default)]
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsersSpec {
    pub users: Vec<User>,
}

/// The `kind: Users` YAML file at `/etc/avix/users.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsersConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: UsersMetadata,
    pub spec: UsersSpec,
}

impl UsersConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self =
            serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn users(&self) -> &[User] {
        &self.spec.users
    }

    pub fn find_user(&self, username: &str) -> Option<&User> {
        self.spec.users.iter().find(|u| u.username == username)
    }

    fn validate(&self) -> Result<(), AvixError> {
        let mut seen_uids = HashSet::new();
        for user in &self.spec.users {
            // Duplicate UID check
            if !seen_uids.insert(user.uid) {
                return Err(AvixError::ConfigParse(format!(
                    "duplicate uid: {}",
                    user.uid
                )));
            }
            // Reserved UID range: 1–999 are reserved for kernel/system agents
            if user.uid > 0 && user.uid < 1000 {
                return Err(AvixError::ConfigParse(format!(
                    "uid {} is in the reserved range 1–999 (user '{}')",
                    user.uid, user.username
                )));
            }
            // workspace XOR home: at most one may be set
            if user.workspace.is_some() && user.home.is_some() {
                return Err(AvixError::ConfigParse(format!(
                    "user '{}' must have workspace OR home, not both",
                    user.username
                )));
            }
            // Validate QuotaValue strings
            if let Some(q) = &user.quota {
                for (label, val) in [
                    ("tokens", &q.tokens),
                    ("agents", &q.agents),
                    ("sessions", &q.sessions),
                ] {
                    if let Some(QuotaValue::Unlimited(s)) = val {
                        if s != "unlimited" {
                            return Err(AvixError::ConfigParse(format!(
                                "user '{}' quota.{label}: invalid value '{s}' \
                                 (expected a positive integer or the string \"unlimited\")",
                                user.username
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn full_yaml() -> &'static str {
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

    #[test]
    fn full_parse() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        assert_eq!(cfg.users().len(), 3);
    }

    #[test]
    fn flat_users_key_fails() {
        let yaml = "apiVersion: avix/v1\nkind: Users\nusers:\n  - username: alice\n    uid: 1001\n";
        assert!(UsersConfig::from_str(yaml).is_err());
    }

    #[test]
    fn find_user_works() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        assert!(cfg.find_user("alice").is_some());
        assert!(cfg.find_user("nonexistent").is_none());
    }

    #[test]
    fn workspace_and_home_fields() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        let root = cfg.find_user("root").unwrap();
        assert_eq!(root.home.as_deref(), Some("/root"));
        assert!(root.workspace.is_none());
        let alice = cfg.find_user("alice").unwrap();
        assert_eq!(alice.workspace.as_deref(), Some("/users/alice/workspace"));
        assert!(alice.home.is_none());
    }

    #[test]
    fn shell_and_service_account() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        let svc = cfg.find_user("svc-pipeline").unwrap();
        assert_eq!(svc.shell, "nologin");
        assert!(svc.is_service_account());
        let alice = cfg.find_user("alice").unwrap();
        assert!(!alice.is_service_account());
    }

    #[test]
    fn crews_field() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        let alice = cfg.find_user("alice").unwrap();
        assert!(alice.crews.contains(&"researchers".to_string()));
        assert!(alice.crews.contains(&"writers".to_string()));
    }

    #[test]
    fn quota_unlimited() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        let root = cfg.find_user("root").unwrap();
        let q = root.quota.as_ref().unwrap();
        assert!(q.tokens.as_ref().unwrap().is_unlimited());
        assert!(q.agents.as_ref().unwrap().is_unlimited());
        assert!(q.sessions.as_ref().unwrap().is_unlimited());
    }

    #[test]
    fn quota_numeric() {
        let cfg = UsersConfig::from_str(full_yaml()).unwrap();
        let alice = cfg.find_user("alice").unwrap();
        let q = alice.quota.as_ref().unwrap();
        assert_eq!(q.tokens.as_ref().unwrap().count(), Some(500_000));
        assert_eq!(q.agents.as_ref().unwrap().count(), Some(5));
        assert_eq!(q.sessions.as_ref().unwrap().count(), Some(4));
    }

    #[test]
    fn duplicate_uid_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
            - username: alice\n    uid: 1001\n  - username: bob\n    uid: 1001\n";
        assert!(UsersConfig::from_str(yaml).is_err());
    }

    #[test]
    fn reserved_uid_range_rejected() {
        let yaml =
            "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n  - username: svc\n    uid: 500\n";
        let err = UsersConfig::from_str(yaml).unwrap_err().to_string();
        assert!(err.contains("reserved"), "expected 'reserved' in: {err}");
    }

    #[test]
    fn uid_zero_allowed() {
        let yaml =
            "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n  - username: root\n    uid: 0\n    home: /root\n";
        assert!(UsersConfig::from_str(yaml).is_ok());
    }

    #[test]
    fn both_workspace_and_home_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
            - username: alice\n    uid: 1001\n    workspace: /users/alice/workspace\n    home: /home/alice\n";
        assert!(UsersConfig::from_str(yaml).is_err());
    }

    #[test]
    fn invalid_quota_string_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Users\nspec:\n  users:\n\
            - username: alice\n    uid: 1001\n    quota:\n      tokens: notanumber\n";
        assert!(UsersConfig::from_str(yaml).is_err());
    }

    #[test]
    fn quota_value_round_trip() {
        let v: QuotaValue = serde_yaml::from_str("unlimited").unwrap();
        assert!(v.is_unlimited());
        let v2: QuotaValue = serde_yaml::from_str("42").unwrap();
        assert_eq!(v2.count(), Some(42));
    }
}
