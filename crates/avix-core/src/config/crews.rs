use crate::error::AvixError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;

// ── CrewMember ────────────────────────────────────────────────────────────────

/// A typed crew member entry.
///
/// YAML representations:
/// - `"*"` → `Wildcard` (all users and all agents)
/// - `"user:alice"` → `User("alice")`
/// - `"agent:researcher"` → `Agent("researcher")`
/// - bare name (e.g. `"root"`) → `User("root")` (backward-compatible)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewMember {
    User(String),
    Agent(String),
    Wildcard,
}

impl CrewMember {
    pub fn matches_user(&self, username: &str) -> bool {
        match self {
            CrewMember::User(u) => u == username,
            CrewMember::Wildcard => true,
            CrewMember::Agent(_) => false,
        }
    }

    pub fn matches_agent(&self, template: &str) -> bool {
        match self {
            CrewMember::Agent(t) => t == template,
            CrewMember::Wildcard => true,
            CrewMember::User(_) => false,
        }
    }
}

impl Serialize for CrewMember {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            CrewMember::Wildcard => s.serialize_str("*"),
            CrewMember::User(u) => s.serialize_str(&format!("user:{u}")),
            CrewMember::Agent(t) => s.serialize_str(&format!("agent:{t}")),
        }
    }
}

impl<'de> Deserialize<'de> for CrewMember {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "*" => CrewMember::Wildcard,
            _ if s.starts_with("user:") => CrewMember::User(s[5..].to_string()),
            _ if s.starts_with("agent:") => CrewMember::Agent(s[6..].to_string()),
            // Bare name: treat as a user (backward-compatible with plain usernames like "root")
            _ => CrewMember::User(s),
        })
    }
}

// ── AgentInheritance ──────────────────────────────────────────────────────────

/// Controls whether agents automatically join this crew when spawned by a member user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AgentInheritance {
    /// Agents auto-join when spawned by a member user.
    #[default]
    Spawn,
    /// Agents must be added to the crew explicitly; no auto-join.
    Explicit,
    /// Agents never join this crew even if spawned by a member.
    #[serde(rename = "none")]
    Never,
}

// ── PipePolicy ────────────────────────────────────────────────────────────────

/// Controls whether intra-crew pipes bypass the ResourceRequest cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PipePolicy {
    /// Pipes between crew members proceed without a ResourceRequest.
    #[default]
    AllowIntraCrew,
    /// A ResourceRequest is required even for intra-crew pipes.
    RequireRequest,
    /// All pipe creation between crew members is denied.
    Deny,
}

// ── Crew ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    /// Human-readable crew name; primary identifier used in users.yaml `crews:` lists.
    pub name: String,
    /// Crew ID integer; 0–999 are reserved for kernel/system crews.
    pub cid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Typed member list: users, agent templates, and/or the `"*"` wildcard.
    #[serde(default)]
    pub members: Vec<CrewMember>,
    /// Whether agents auto-join when spawned by a member user.
    #[serde(default)]
    pub agent_inheritance: AgentInheritance,
    /// Base tool set granted to agents spawned by crew members.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tools explicitly blocked, even if user ACL would otherwise allow them.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// VFS paths (under `/crews/<name>/shared/`) where all members have read-write access.
    #[serde(default)]
    pub shared_paths: Vec<String>,
    /// Whether intra-crew pipes bypass the ResourceRequest cycle.
    #[serde(default)]
    pub pipe_policy: PipePolicy,
}

impl Crew {
    pub fn contains_user(&self, username: &str) -> bool {
        self.members.iter().any(|m| m.matches_user(username))
    }

    pub fn contains_agent(&self, template: &str) -> bool {
        self.members.iter().any(|m| m.matches_agent(template))
    }
}

// ── CrewsConfig envelope ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CrewsMetadata {
    #[serde(default)]
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewsSpec {
    pub crews: Vec<Crew>,
}

/// The `kind: Crews` YAML file at `/etc/avix/crews.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewsConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: CrewsMetadata,
    pub spec: CrewsSpec,
}

impl CrewsConfig {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        let cfg: Self =
            serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn crews(&self) -> &[Crew] {
        &self.spec.crews
    }

    pub fn find_crew(&self, name: &str) -> Option<&Crew> {
        self.spec.crews.iter().find(|c| c.name == name)
    }

    /// Returns all crews that the given username belongs to (including wildcard crews).
    pub fn crews_for_user(&self, username: &str) -> Vec<&Crew> {
        self.spec
            .crews
            .iter()
            .filter(|c| c.contains_user(username))
            .collect()
    }

    /// Returns all crews that the given agent template belongs to (including wildcard crews).
    pub fn crews_for_agent(&self, template: &str) -> Vec<&Crew> {
        self.spec
            .crews
            .iter()
            .filter(|c| c.contains_agent(template))
            .collect()
    }

    fn validate(&self) -> Result<(), AvixError> {
        let mut seen_cids: HashSet<u32> = HashSet::new();
        let mut seen_names: HashSet<&str> = HashSet::new();
        for crew in &self.spec.crews {
            if !seen_names.insert(crew.name.as_str()) {
                return Err(AvixError::ConfigParse(format!(
                    "duplicate crew name: {}",
                    crew.name
                )));
            }
            if !seen_cids.insert(crew.cid) {
                return Err(AvixError::ConfigParse(format!(
                    "duplicate cid: {}",
                    crew.cid
                )));
            }
            // Validate sharedPaths must be under /crews/<name>/shared/
            let prefix = format!("/crews/{}/shared/", crew.name);
            for path in &crew.shared_paths {
                if !path.starts_with(&prefix) {
                    return Err(AvixError::ConfigParse(format!(
                        "crew '{}' sharedPaths entry '{}' must be under '{}'",
                        crew.name, path, prefix
                    )));
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
kind: Crews
metadata:
  lastUpdated: "2026-03-20T00:00:00Z"
spec:
  crews:
    - name: all
      cid: 0
      description: Every user; world-readable access baseline
      members: ["*"]

    - name: researchers
      cid: 1001
      description: Human researchers and researcher-template agents
      members:
        - user:alice
        - agent:researcher
      agentInheritance: spawn
      allowedTools:
        - web_search
        - file_read
      deniedTools:
        - bash
      sharedPaths:
        - /crews/researchers/shared/research/
      pipePolicy: allow-intra-crew

    - name: automation
      cid: 2001
      description: Headless service accounts and scheduled agents
      members:
        - user:svc-pipeline
        - agent:pipeline-ingest
      agentInheritance: none
      allowedTools:
        - web_fetch
        - python
      deniedTools:
        - bash
      sharedPaths:
        - /crews/automation/shared/pipeline/
      pipePolicy: require-request
"#
    }

    #[test]
    fn full_parse() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        assert_eq!(cfg.crews().len(), 3);
    }

    #[test]
    fn flat_crews_key_fails() {
        let yaml = "apiVersion: avix/v1\nkind: Crews\ncrews:\n  - name: all\n    cid: 0\n";
        assert!(CrewsConfig::from_str(yaml).is_err());
    }

    #[test]
    fn name_and_cid_fields() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let r = cfg.find_crew("researchers").unwrap();
        assert_eq!(r.cid, 1001u32);
        assert_eq!(r.name, "researchers");
    }

    #[test]
    fn typed_user_and_agent_members() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let r = cfg.find_crew("researchers").unwrap();
        assert!(r.contains_user("alice"));
        assert!(r.contains_agent("researcher"));
        assert!(!r.contains_user("bob"));
        assert!(!r.contains_agent("writer"));
    }

    #[test]
    fn wildcard_member_matches_anything() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let all = cfg.find_crew("all").unwrap();
        assert!(all.contains_user("anyone"));
        assert!(all.contains_agent("any-template"));
    }

    #[test]
    fn agent_inheritance_fields() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        assert_eq!(
            cfg.find_crew("researchers").unwrap().agent_inheritance,
            AgentInheritance::Spawn
        );
        assert_eq!(
            cfg.find_crew("automation").unwrap().agent_inheritance,
            AgentInheritance::Never
        );
    }

    #[test]
    fn pipe_policy_fields() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        assert_eq!(
            cfg.find_crew("researchers").unwrap().pipe_policy,
            PipePolicy::AllowIntraCrew
        );
        assert_eq!(
            cfg.find_crew("automation").unwrap().pipe_policy,
            PipePolicy::RequireRequest
        );
    }

    #[test]
    fn shared_paths_field() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let r = cfg.find_crew("researchers").unwrap();
        assert!(r
            .shared_paths
            .contains(&"/crews/researchers/shared/research/".to_string()));
    }

    #[test]
    fn crews_for_user() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let alice_crews = cfg.crews_for_user("alice");
        assert!(alice_crews.iter().any(|c| c.name == "researchers"));
        assert!(alice_crews.iter().any(|c| c.name == "all")); // wildcard
        assert!(!alice_crews.iter().any(|c| c.name == "automation"));
    }

    #[test]
    fn crews_for_agent() {
        let cfg = CrewsConfig::from_str(full_yaml()).unwrap();
        let r_crews = cfg.crews_for_agent("researcher");
        assert!(r_crews.iter().any(|c| c.name == "researchers"));
        assert!(r_crews.iter().any(|c| c.name == "all")); // wildcard
    }

    #[test]
    fn duplicate_cid_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
            - name: a\n    cid: 1001\n  - name: b\n    cid: 1001\n";
        assert!(CrewsConfig::from_str(yaml).is_err());
    }

    #[test]
    fn duplicate_name_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
            - name: dup\n    cid: 1001\n  - name: dup\n    cid: 1002\n";
        assert!(CrewsConfig::from_str(yaml).is_err());
    }

    #[test]
    fn invalid_shared_path_rejected() {
        let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
            - name: bad\n    cid: 1001\n    sharedPaths:\n      - /etc/avix/\n";
        assert!(CrewsConfig::from_str(yaml).is_err());
    }

    #[test]
    fn bare_username_treated_as_user_member() {
        let yaml = r#"
apiVersion: avix/v1
kind: Crews
spec:
  crews:
    - name: kernel
      cid: 1
      members:
        - root
"#;
        let cfg = CrewsConfig::from_str(yaml).unwrap();
        assert!(cfg.find_crew("kernel").unwrap().contains_user("root"));
    }

    #[test]
    fn crew_member_serialise_round_trip() {
        let members = vec![
            CrewMember::Wildcard,
            CrewMember::User("alice".into()),
            CrewMember::Agent("researcher".into()),
        ];
        let yaml = serde_yaml::to_string(&members).unwrap();
        let parsed: Vec<CrewMember> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, members);
    }

    #[test]
    fn empty_crews_list_parses() {
        let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews: []\n";
        let cfg = CrewsConfig::from_str(yaml).unwrap();
        assert_eq!(cfg.crews().len(), 0);
    }
}
