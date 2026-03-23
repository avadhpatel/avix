# Crews Gap A — Full Schema Alignment

> **Status:** Not started
> **Priority:** High — crews.yaml affects tool grant resolution and is referenced by users.yaml
> **Depends on:** None (self-contained to `crates/avix-core/src/config/crews.rs`)
> **Affects:** `avix-core/src/config/crews.rs`, `avix-core/src/cli/config_init.rs`, `avix-core/tests/config.rs`

---

## Problem

The current `CrewsConfig` / `Crew` structs are structurally incompatible with the spec in five ways:

1. **Structure mismatch.** Spec uses `spec.crews[]` (with `spec:` wrapper and `metadata:`). `CrewsConfig` currently parses a flat `crews:` key. Same bug as `UsersConfig` — the `config_init` template writes `spec.crews:` which `CrewsConfig::from_str()` would silently ignore.

2. **Missing `name` field.** Each crew entry has a `name:` (human-readable string) separate from `cid:` (integer). The current `Crew` struct has only `cid: String` and uses it as both identifier and name.

3. **`cid` is `String` instead of `u32`.** The spec defines `cid` as an integer; CIDs 0–999 are reserved. The current struct uses `String`, which makes range validation impossible.

4. **Missing fields.** `description`, `agentInheritance`, `sharedPaths`, and `pipePolicy` are all absent.

5. **Untyped `members`.** The spec defines three member kinds — `user:<name>`, `agent:<template>`, and `"*"` (wildcard). The current `Vec<String>` loses that structure and makes membership evaluation impossible.

---

## What Needs to Be Built

### `CrewMember` — typed member enum

```rust
/// A typed crew member entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewMember {
    /// A specific human user: `user:alice`
    User(String),
    /// Any running instance of an agent template: `agent:researcher`
    Agent(String),
    /// Wildcard: all users and all agents. Serialises as `"*"`.
    Wildcard,
}

impl CrewMember {
    pub fn matches_user(&self, username: &str) -> bool {
        matches!(self, CrewMember::User(u) if u == username)
            || matches!(self, CrewMember::Wildcard)
    }
    pub fn matches_agent(&self, template: &str) -> bool {
        matches!(self, CrewMember::Agent(t) if t == template)
            || matches!(self, CrewMember::Wildcard)
    }
}
```

Custom `Serialize`/`Deserialize`:
- `"*"` → `CrewMember::Wildcard`
- `"user:alice"` → `CrewMember::User("alice")`
- `"agent:researcher"` → `CrewMember::Agent("researcher")`
- bare name (e.g. `"root"`) → `CrewMember::User("root")` (backward-compatibility)

Serialise back as:
- `Wildcard` → `"*"`
- `User(n)` → `"user:<n>"`
- `Agent(t)` → `"agent:<t>"`

### `AgentInheritance` — enum

```rust
/// Controls whether agents auto-join this crew when spawned by a member user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AgentInheritance {
    /// Agents automatically join when spawned by a member user.
    #[default]
    Spawn,
    /// Agents must be added explicitly; no auto-join.
    Explicit,
    /// Agents never join this crew, even if spawned by a member.
    None,
}
```

### `PipePolicy` — enum

```rust
/// Controls whether intra-crew pipes bypass the ResourceRequest cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PipePolicy {
    /// Pipes between crew members proceed without a ResourceRequest.
    #[default]
    AllowIntraCrew,
    /// A ResourceRequest is required even for intra-crew pipes.
    RequireRequest,
    /// All pipe creation between members of this crew is denied.
    Deny,
}
```

### `Crew` — full spec-compliant struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    /// Human-readable crew name. Used as the primary identifier in users.yaml `crews:` lists.
    pub name: String,
    /// Crew ID integer; 0–999 are reserved for kernel/system crews.
    pub cid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Typed member list.
    #[serde(default)]
    pub members: Vec<CrewMember>,
    /// Whether agents auto-join when spawned by a member user.
    #[serde(default)]
    pub agent_inheritance: AgentInheritance,
    /// Base tool set granted to agents spawned by crew members.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tools explicitly blocked even if user ACL would allow them.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// VFS paths under `/crews/<name>/shared/` where all members have read-write access.
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
```

### `CrewsMetadata`, `CrewsSpec`, `CrewsConfig` wrappers

```rust
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

pub struct CrewsConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: CrewsMetadata,
    pub spec: CrewsSpec,
}

impl CrewsConfig {
    pub fn crews(&self) -> &[Crew] { &self.spec.crews }
    pub fn find_crew(&self, name: &str) -> Option<&Crew> {
        self.spec.crews.iter().find(|c| c.name == name)
    }
    pub fn crews_for_user(&self, username: &str) -> Vec<&Crew> {
        self.spec.crews.iter().filter(|c| c.contains_user(username)).collect()
    }
}
```

### Validation rules (updated)

```rust
fn validate(&self) -> Result<(), AvixError> {
    let mut seen_cids = HashSet::new();
    let mut seen_names = HashSet::new();
    for crew in &self.spec.crews {
        if !seen_names.insert(&crew.name) {
            return Err(AvixError::ConfigParse(format!("duplicate crew name: {}", crew.name)));
        }
        if !seen_cids.insert(crew.cid) {
            return Err(AvixError::ConfigParse(format!("duplicate cid: {}", crew.cid)));
        }
        // Validate sharedPaths are under /crews/<name>/shared/
        let prefix = format!("/crews/{}/shared/", crew.name);
        for path in &crew.shared_paths {
            if !path.starts_with(&prefix) {
                return Err(AvixError::ConfigParse(format!(
                    "crew '{}' sharedPath '{}' must start with '{}'",
                    crew.name, path, prefix
                )));
            }
        }
    }
    Ok(())
}
```

### Updated `config_init` template (`CREWS_YAML_TEMPLATE`)

```yaml
apiVersion: avix/v1
kind: Crews
metadata:
  lastUpdated: "{now}"

spec:
  crews: []
```

> The template keeps `crews: []` by design — the admin must manually define crews after init.
> For documentation purposes, the `config_init` output should note where to add crews.

### Updated `USERS_YAML_TEMPLATE` cross-reference

The `config_init` template for `users.yaml` already includes a `crews: []` field on the user entry (per users-gap-A). No change needed here.

---

## TDD Test Plan

File: `crates/avix-core/tests/config.rs` (replace existing CrewsConfig tests)

```rust
fn crews_yaml_full() -> &'static str {
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
      description: Human researchers and any researcher-template agents
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
      description: Headless service accounts
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

// T-CA-01: Full spec-compliant YAML parses
#[test]
fn crews_config_full_parse() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    assert_eq!(cfg.crews().len(), 3);
}

// T-CA-02: spec wrapper is required (flat crews: fails)
#[test]
fn crews_config_flat_crews_fails() {
    let yaml = "apiVersion: avix/v1\nkind: Crews\ncrews:\n  - name: all\n    cid: 0\n";
    assert!(CrewsConfig::from_str(yaml).is_err());
}

// T-CA-03: name and cid fields
#[test]
fn crews_config_name_and_cid() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    let r = cfg.find_crew("researchers").unwrap();
    assert_eq!(r.cid, 1001u32);
    assert_eq!(r.name, "researchers");
}

// T-CA-04: typed member parsing
#[test]
fn crews_config_typed_members() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    let r = cfg.find_crew("researchers").unwrap();
    assert!(r.contains_user("alice"));
    assert!(r.contains_agent("researcher"));
    assert!(!r.contains_user("bob"));
}

// T-CA-05: wildcard member matches any user/agent
#[test]
fn crews_config_wildcard_member() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    let all = cfg.find_crew("all").unwrap();
    assert!(all.contains_user("anyone"));
    assert!(all.contains_agent("any-template"));
}

// T-CA-06: agentInheritance field
#[test]
fn crews_config_agent_inheritance() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    assert_eq!(cfg.find_crew("researchers").unwrap().agent_inheritance, AgentInheritance::Spawn);
    assert_eq!(cfg.find_crew("automation").unwrap().agent_inheritance, AgentInheritance::None);
}

// T-CA-07: pipePolicy field
#[test]
fn crews_config_pipe_policy() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    assert_eq!(cfg.find_crew("researchers").unwrap().pipe_policy, PipePolicy::AllowIntraCrew);
    assert_eq!(cfg.find_crew("automation").unwrap().pipe_policy, PipePolicy::RequireRequest);
}

// T-CA-08: sharedPaths field
#[test]
fn crews_config_shared_paths() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    let r = cfg.find_crew("researchers").unwrap();
    assert!(r.shared_paths.contains(&"/crews/researchers/shared/research/".to_string()));
}

// T-CA-09: crews_for_user returns correct crews
#[test]
fn crews_config_crews_for_user() {
    let cfg = CrewsConfig::from_str(crews_yaml_full()).unwrap();
    let alice_crews = cfg.crews_for_user("alice");
    assert!(alice_crews.iter().any(|c| c.name == "researchers"));
    assert!(alice_crews.iter().any(|c| c.name == "all")); // wildcard
    assert!(!alice_crews.iter().any(|c| c.name == "automation"));
}

// T-CA-10: duplicate CID rejected
#[test]
fn crews_config_rejects_duplicate_cid() {
    let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
        - name: a\n    cid: 1001\n  - name: b\n    cid: 1001\n";
    assert!(CrewsConfig::from_str(yaml).is_err());
}

// T-CA-11: duplicate name rejected
#[test]
fn crews_config_rejects_duplicate_name() {
    let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
        - name: dup\n    cid: 1001\n  - name: dup\n    cid: 1002\n";
    assert!(CrewsConfig::from_str(yaml).is_err());
}

// T-CA-12: sharedPaths outside /crews/<name>/shared/ rejected
#[test]
fn crews_config_rejects_invalid_shared_paths() {
    let yaml = "apiVersion: avix/v1\nkind: Crews\nspec:\n  crews:\n\
        - name: bad\n    cid: 1001\n    sharedPaths:\n      - /etc/avix/\n";
    assert!(CrewsConfig::from_str(yaml).is_err());
}

// T-CA-13: config_init template is spec-compliant
#[test]
fn crews_config_init_template_parses() {
    use avix_core::cli::config_init::{run_config_init, ConfigInitParams};
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    run_config_init(ConfigInitParams {
        root: dir.path().to_path_buf(), identity_name: "alice".into(),
        credential_type: "api_key".into(), role: "admin".into(),
        master_key_source: "env".into(), mode: "cli".into(),
    }).unwrap();
    let yaml = std::fs::read_to_string(dir.path().join("etc/crews.yaml")).unwrap();
    let cfg = CrewsConfig::from_str(&yaml).unwrap();
    assert_eq!(cfg.crews().len(), 0); // starts empty
}
```

---

## Implementation Notes

- `CrewMember` custom serde: implement `Deserialize` manually. Parse the YAML string value: if `"*"` → Wildcard; if starts with `"user:"` → User(suffix); if starts with `"agent:"` → Agent(suffix); else → User(whole string) for backward-compat with bare names like `"root"`.
- `AgentInheritance::None` conflicts with Rust's keyword `None` — use `#[serde(rename = "none")]` on the variant.
- Remove existing `cid: String` test (`crew.cid == "research-crew"`) — replace with new tests above.
- The existing tests `crews_config_parses_successfully`, `crews_config_members`, and `crews_config_allowed_denied_tools` must all be replaced.
- `CrewsConfig::crews_for_user()` should be called from `avix resolve` (via `config/crews.rs`) rather than computing crew membership inline in `resolve.rs`.

---

## Success Criteria

- [ ] Full spec YAML parses (T-CA-01)
- [ ] Flat `crews:` at root level fails (T-CA-02)
- [ ] `name` (String) and `cid` (u32) both present (T-CA-03)
- [ ] Typed `CrewMember` parsing: `user:`, `agent:`, `"*"` (T-CA-04, T-CA-05)
- [ ] `agentInheritance` parsed correctly (T-CA-06)
- [ ] `pipePolicy` parsed correctly (T-CA-07)
- [ ] `sharedPaths` persisted (T-CA-08)
- [ ] `crews_for_user()` works (T-CA-09)
- [ ] Duplicate CID rejected (T-CA-10)
- [ ] Duplicate name rejected (T-CA-11)
- [ ] Invalid `sharedPaths` rejected (T-CA-12)
- [ ] `config_init` template parses (T-CA-13)
- [ ] `cargo clippy --workspace -- -D warnings` passes
