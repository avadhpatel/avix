# Session Manifest Gap A — SessionEntry Alignment with Spec

> **Status:** Not started
> **Priority:** Medium — VFS manifest output is already written; this aligns the struct fields
> **Depends on:** None
> **Affects:** `avix-core/src/session/entry.rs`, `avix-core/src/session/store.rs`, `avix-core/tests/session.rs`

---

## Problem

`SessionEntry` exists and is persisted to VFS as `/proc/users/<username>/sessions/<session-id>.yaml`,
but the struct does not match `SessionManifest` spec. The following are missing or misaligned:

| Spec field | Current state |
|---|---|
| `spec.shell` | Missing |
| `spec.tty` | Missing |
| `spec.workingDirectory` | Missing |
| `spec.agents` (list with pid + name + role) | Only `agent_name: String` (single agent, no pid, no role) |
| `spec.quotaSnapshot` (tokensUsed/Limit, agentsRunning/Limit) | Missing |
| `status.state: active \| idle \| closed` | `SessionStatus` has `Active/Completed/Error` — wrong variants |
| `status.lastActivityAt` | Missing |
| `status.closedAt` | Missing |
| `status.closedReason` | Missing |

The VFS manifest written by `store.rs` inherits all of these gaps, so the on-disk YAML does not
match the spec schema either.

Additionally, there is no mechanism to:
- Track multiple agents per session
- Record quota state at session open
- Transition a session to `idle` or `closed`
- Record a close reason

---

## What Needs to Be Built

### 1. `SessionState` enum — replace `SessionStatus`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Active,
    Idle,
    Closed,
}
```

The old `SessionStatus::Completed` and `SessionStatus::Error` are not in the spec. Remove them.
`Closed` is the terminal state (covers both normal completion and error termination — the reason
is in `closed_reason`).

> **Migration note:** Existing tests referencing `SessionStatus::Completed` or
> `SessionStatus::Error` must be updated to use `SessionState::Closed` with an appropriate
> `closed_reason` string.

### 2. `AgentRef` — a single agent entry in `spec.agents`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRef {
    pub pid: u32,
    pub name: String,
    pub role: AgentRole,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    #[default]
    Primary,
    Subordinate,
}
```

### 3. `QuotaSnapshot` — point-in-time quota at session open

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    #[serde(default)]
    pub tokens_used: u64,
    pub tokens_limit: u64,
    #[serde(default)]
    pub agents_running: u32,
    pub agents_limit: u32,
}
```

`tokens_used` and `agents_running` default to 0 (a new session has consumed nothing).
`tokens_limit` and `agents_limit` come from the user's quota in `/etc/avix/users.yaml`.

### 4. Revised `SessionEntry`

Replace the existing struct entirely:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub session_id: String,
    pub username: String,
    pub uid: u32,

    // spec fields
    pub shell: String,                           // default: "/bin/sh"
    pub tty: bool,                               // default: true
    pub working_directory: String,               // default: /users/<username>/workspace

    pub agents: Vec<AgentRef>,                   // grows as agents are spawned into session
    pub quota_snapshot: QuotaSnapshot,           // captured at session open

    // status fields
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_reason: Option<String>,

    // internal (not in VFS manifest — stored in redb only)
    pub messages: Vec<serde_json::Value>,
}
```

`messages` is internal runtime state (conversation history). It is stored in redb but
NOT written to the VFS manifest — the VFS file is the observable status view, not a
conversation archive.

### 5. `SessionEntry` constructor helpers

```rust
impl SessionEntry {
    /// Create a new session with sensible defaults.
    pub fn new(
        session_id: String,
        username: String,
        uid: u32,
        quota_snapshot: QuotaSnapshot,
    ) -> Self {
        let now = Utc::now();
        let working_directory = format!("/users/{}/workspace", username);
        Self {
            session_id,
            username: username.clone(),
            uid,
            shell: "/bin/sh".into(),
            tty: true,
            working_directory,
            agents: vec![],
            quota_snapshot,
            state: SessionState::Active,
            created_at: now,
            last_activity_at: now,
            closed_at: None,
            closed_reason: None,
            messages: vec![],
        }
    }

    /// Attach an agent to this session.
    pub fn add_agent(&mut self, pid: u32, name: String, role: AgentRole) {
        self.agents.push(AgentRef { pid, name, role });
        self.last_activity_at = Utc::now();
    }

    /// Close the session with a reason.
    pub fn close(&mut self, reason: impl Into<String>) {
        let now = Utc::now();
        self.state = SessionState::Closed;
        self.closed_at = Some(now);
        self.closed_reason = Some(reason.into());
        self.last_activity_at = now;
    }

    /// Mark session as idle.
    pub fn mark_idle(&mut self) {
        self.state = SessionState::Idle;
        self.last_activity_at = Utc::now();
    }

    /// Return to active from idle.
    pub fn mark_active(&mut self) {
        self.state = SessionState::Active;
        self.last_activity_at = Utc::now();
    }
}
```

### 6. Update `store.rs` — `write_vfs_manifest()`

The VFS manifest emitted by `SessionStore::write_vfs_manifest` must match the spec schema:

```yaml
apiVersion: avix/v1
kind: SessionManifest
metadata:
  sessionId: sess-alice-001
  createdAt: 2026-03-15T07:30:00Z
  user: alice
  uid: 1001

spec:
  shell: /bin/sh
  tty: true
  workingDirectory: /users/alice/workspace
  agents:
    - pid: 57
      name: researcher
      role: primary
  quotaSnapshot:
    tokensUsed: 0
    tokensLimit: 500000
    agentsRunning: 0
    agentsLimit: 5

status:
  state: active
  lastActivityAt: 2026-03-15T07:41:12Z
  closedAt: null
  closedReason: null
```

In the `write_vfs_manifest` method, build this struct and serialise with `serde_yaml`.
The `messages` field is **excluded** — it is redb-only.

---

## TDD Test Plan

File: `crates/avix-core/tests/session.rs` — update and extend existing tests.

```rust
// T-SMA-01: SessionEntry::new creates correct defaults
#[test]
fn session_entry_new_defaults() {
    let quota = QuotaSnapshot { tokens_limit: 500_000, agents_limit: 5, ..Default::default() };
    let entry = SessionEntry::new("sess-001".into(), "alice".into(), 1001, quota);
    assert_eq!(entry.shell, "/bin/sh");
    assert!(entry.tty);
    assert_eq!(entry.working_directory, "/users/alice/workspace");
    assert!(entry.agents.is_empty());
    assert_eq!(entry.state, SessionState::Active);
    assert!(entry.closed_at.is_none());
    assert!(entry.closed_reason.is_none());
}

// T-SMA-02: add_agent attaches AgentRef and updates last_activity_at
#[test]
fn add_agent_appends_agent_ref() {
    let mut entry = make_test_session();
    let before = entry.last_activity_at;
    std::thread::sleep(std::time::Duration::from_millis(1));
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    assert_eq!(entry.agents.len(), 1);
    assert_eq!(entry.agents[0].pid, 57);
    assert_eq!(entry.agents[0].role, AgentRole::Primary);
    assert!(entry.last_activity_at >= before);
}

// T-SMA-03: close() sets state, closed_at, closed_reason
#[test]
fn close_sets_terminal_state() {
    let mut entry = make_test_session();
    entry.close("user logged out");
    assert_eq!(entry.state, SessionState::Closed);
    assert!(entry.closed_at.is_some());
    assert_eq!(entry.closed_reason.as_deref(), Some("user logged out"));
}

// T-SMA-04: mark_idle / mark_active cycle
#[test]
fn idle_active_cycle() {
    let mut entry = make_test_session();
    entry.mark_idle();
    assert_eq!(entry.state, SessionState::Idle);
    entry.mark_active();
    assert_eq!(entry.state, SessionState::Active);
}

// T-SMA-05: SessionEntry round-trips through redb save/load
#[tokio::test]
async fn session_entry_round_trips_redb() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::open(dir.path().join("sessions.db")).await.unwrap();
    let mut entry = make_test_session_with_uid(1001);
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    entry.quota_snapshot = QuotaSnapshot { tokens_limit: 500_000, agents_limit: 5, ..Default::default() };
    store.save(&entry).await.unwrap();
    let loaded = store.load(&entry.session_id).await.unwrap().unwrap();
    assert_eq!(loaded.agents.len(), 1);
    assert_eq!(loaded.agents[0].pid, 57);
    assert_eq!(loaded.quota_snapshot.tokens_limit, 500_000);
    assert_eq!(loaded.state, SessionState::Active);
}

// T-SMA-06: VFS manifest contains spec-compliant fields
#[tokio::test]
async fn vfs_manifest_matches_spec_schema() {
    let dir = tempfile::tempdir().unwrap();
    let (store, vfs) = build_test_store_with_vfs(dir.path()).await;
    let mut entry = make_test_session_with_uid(1001);
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    entry.quota_snapshot = QuotaSnapshot {
        tokens_used: 0, tokens_limit: 500_000,
        agents_running: 1, agents_limit: 5,
    };
    store.save(&entry).await.unwrap();

    let path = format!("/proc/users/{}/sessions/{}.yaml", entry.username, entry.session_id);
    let raw = vfs.read(&path).await.unwrap();
    let content = String::from_utf8(raw).unwrap();

    assert!(content.contains("kind: SessionManifest"));
    assert!(content.contains("shell: /bin/sh"));
    assert!(content.contains("tty: true"));
    assert!(content.contains("workingDirectory:"));
    assert!(content.contains("pid: 57"));
    assert!(content.contains("role: primary"));
    assert!(content.contains("tokensLimit: 500000"));
    assert!(content.contains("agentsLimit: 5"));
    assert!(content.contains("state: active"));
    assert!(content.contains("lastActivityAt:"));
    // messages must NOT be in the VFS manifest
    assert!(!content.contains("messages:"));
}

// T-SMA-07: VFS manifest updated when session is closed
#[tokio::test]
async fn vfs_manifest_updated_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let (store, vfs) = build_test_store_with_vfs(dir.path()).await;
    let mut entry = make_test_session_with_uid(1001);
    store.save(&entry).await.unwrap();

    entry.close("test close");
    store.save(&entry).await.unwrap();

    let path = format!("/proc/users/{}/sessions/{}.yaml", entry.username, entry.session_id);
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();
    assert!(content.contains("state: closed"));
    assert!(content.contains("closedAt:"));
    assert!(content.contains("closedReason: test close"));
}

// T-SMA-08: Multiple agents serialise correctly in VFS manifest
#[tokio::test]
async fn multiple_agents_in_vfs_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let (store, vfs) = build_test_store_with_vfs(dir.path()).await;
    let mut entry = make_test_session_with_uid(1001);
    entry.add_agent(57, "researcher".into(), AgentRole::Primary);
    entry.add_agent(58, "writer".into(), AgentRole::Subordinate);
    store.save(&entry).await.unwrap();

    let path = format!("/proc/users/{}/sessions/{}.yaml", entry.username, entry.session_id);
    let content = String::from_utf8(vfs.read(&path).await.unwrap()).unwrap();
    assert!(content.contains("pid: 57"));
    assert!(content.contains("role: primary"));
    assert!(content.contains("pid: 58"));
    assert!(content.contains("role: subordinate"));
}

// T-SMA-09: SessionState serialises to lowercase
#[test]
fn session_state_serialises_lowercase() {
    assert_eq!(serde_yaml::to_string(&SessionState::Active).unwrap().trim(), "active");
    assert_eq!(serde_yaml::to_string(&SessionState::Idle).unwrap().trim(), "idle");
    assert_eq!(serde_yaml::to_string(&SessionState::Closed).unwrap().trim(), "closed");
}

// T-SMA-10: AgentRole serialises to lowercase
#[test]
fn agent_role_serialises_lowercase() {
    assert_eq!(serde_yaml::to_string(&AgentRole::Primary).unwrap().trim(), "primary");
    assert_eq!(serde_yaml::to_string(&AgentRole::Subordinate).unwrap().trim(), "subordinate");
}
```

---

## Implementation Notes

- The `messages` field is redb-only. Build the VFS YAML from a separate view struct that omits
  it — do NOT add `#[serde(skip)]` to `messages` in `SessionEntry` itself (it must survive
  redb round-trips). Instead, define a private `SessionManifestView` struct that mirrors the
  spec schema and is used only in `write_vfs_manifest()`.
- The `uid` field: existing `SessionEntry` does not have it. Add it as `uid: u32` with a
  `#[serde(default)]` so that existing redb records with no `uid` deserialise to 0.
- `SessionState` replaces `SessionStatus`. Rename the type and update all call sites.
  The redb encoding serialises as JSON — existing records with `"status":"active"` (snake_case)
  will need `#[serde(alias = "status")]` handling OR the existing tests that write `SessionStatus`
  values must be rewritten from scratch (preferred, since there are no prod records).
- `last_activity_at` is initialised to `created_at` in `SessionEntry::new`. It is updated
  by `add_agent()`, `mark_idle()`, `mark_active()`, `close()`, and whenever `messages` are
  appended (caller's responsibility).
- `add_agent()` does not enforce uniqueness. The caller (kernel spawn code) is responsible for
  not adding the same PID twice.
- The `tty` field default is `true` (CLI sessions are TTY). API/headless callers must explicitly
  set `entry.tty = false`.

---

## Success Criteria

- [ ] `SessionEntry::new` produces spec-compliant defaults (T-SMA-01)
- [ ] `add_agent` correctly appends `AgentRef` and updates `last_activity_at` (T-SMA-02)
- [ ] `close()` transitions to `Closed` state with `closed_at` + `closed_reason` (T-SMA-03)
- [ ] `mark_idle` / `mark_active` cycle works correctly (T-SMA-04)
- [ ] `SessionEntry` with agents and quota survives redb round-trip (T-SMA-05)
- [ ] VFS manifest contains all spec-required fields and omits `messages` (T-SMA-06)
- [ ] VFS manifest reflects closed state after `close()` + `save()` (T-SMA-07)
- [ ] Multiple agents serialise correctly in VFS manifest (T-SMA-08)
- [ ] `SessionState` serialises to lowercase (T-SMA-09)
- [ ] `AgentRole` serialises to lowercase (T-SMA-10)
- [ ] All existing session tests updated / passing
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
