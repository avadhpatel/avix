# Snapshot Gap A — Schema Alignment

> **Status:** Not started
> **Priority:** High — current Snapshot struct is incompatible with spec; all downstream gaps depend on this
> **Depends on:** None
> **Affects:** `avix-core/src/snapshot/capture.rs`, `avix-core/src/snapshot/store.rs`, `avix-core/src/snapshot/mod.rs`

---

## Problem

The current `Snapshot` struct is an internal data model that does not match the spec YAML envelope at all:

| Spec field | Spec location | Implementation | Status |
|---|---|---|---|
| `apiVersion`, `kind` | root | missing | ❌ |
| `metadata.name` | `<agent>-<timestamp>` string | `meta.id` (UUID, not readable) | ❌ |
| `metadata.agentName` | string | `meta.agent_name` | ~✓ |
| `metadata.sourcePid` | integer | `meta.agent_pid` | ~✓ |
| `metadata.capturedAt` | ISO 8601 | `meta.created_at` | ~✓ |
| `metadata.capturedBy` | `kernel` / `user:<uid>` / `agent:<pid>` | `meta.spawned_by` (wrong meaning) | ❌ |
| `metadata.trigger` | `auto/crash/manual/sigsave` | missing | ❌ |
| `spec.goal` | string | `meta.goal` | ~✓ |
| `spec.contextSummary` | string | missing | ❌ |
| `spec.contextTokenCount` | integer | missing | ❌ |
| `spec.memory.episodicEvents` | integer | missing | ❌ |
| `spec.memory.semanticKeys` | integer | missing | ❌ |
| `spec.pendingRequests[]` | in-flight requests | missing | ❌ |
| `spec.pipes[]` | open pipe state | missing | ❌ |
| `spec.environment.temperature` | float | missing | ❌ |
| `spec.environment.capabilityToken` | sha256 string | missing | ❌ |
| `spec.checksum` | sha256 string | missing | ❌ |

The `SnapshotStore` is also purely in-memory with no YAML serialisation capability. Every downstream gap (capture, restore) needs the correct struct first.

---

## What Needs to Be Built

### `SnapshotTrigger` enum

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotTrigger {
    Auto,
    Crash,
    #[default]
    Manual,
    Sigsave,
}
```

### `CapturedBy` — custom typed enum

```rust
/// Who triggered the snapshot capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturedBy {
    /// Kernel triggered (e.g. SIGSAVE, auto-interval, crash).
    Kernel,
    /// A specific human user (UID).
    User(u32),
    /// An agent (PID) triggered via `snap/save` syscall.
    Agent(u32),
}
```

Serde: serialise as `"kernel"`, `"user:1001"`, `"agent:57"`. Deserialise by splitting on `:`.

### `SnapshotMetadata`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMetadata {
    /// Human-readable name: `<agentName>-<YYYYMMDD>-<HHMM>`.
    pub name: String,
    /// Template name of the agent.
    pub agent_name: String,
    /// PID of the agent at capture time.
    pub source_pid: u32,
    /// When the snapshot was taken.
    pub captured_at: chrono::DateTime<chrono::Utc>,
    /// Who triggered the capture.
    pub captured_by: CapturedBy,
    /// What caused the capture.
    pub trigger: SnapshotTrigger,
}
```

### `SnapshotMemory`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMemory {
    pub episodic_events: u32,
    pub semantic_keys: u32,
}
```

### `PendingRequest`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub request_id: String,
    pub resource: String,
    pub name: String,
    /// Always `"in-flight"` for requests captured mid-execution.
    pub status: String,
}
```

### `SnapshotPipe`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPipe {
    pub pipe_id: String,
    /// Always `"open"` for pipes captured mid-execution.
    pub state: String,
}
```

### `SnapshotEnvironment`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotEnvironment {
    pub temperature: f32,
    /// SHA-256 signature of the capability token at capture time.
    /// Used on restore to derive the original capability set.
    pub capability_token: String,
}
```

### `SnapshotSpec`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSpec {
    pub goal: String,
    pub context_summary: String,
    pub context_token_count: u32,
    #[serde(default)]
    pub memory: SnapshotMemory,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    #[serde(default)]
    pub pipes: Vec<SnapshotPipe>,
    pub environment: SnapshotEnvironment,
    /// SHA-256 integrity hash over the canonical YAML of this snapshot (excluding the checksum field itself).
    pub checksum: String,
}
```

### `SnapshotFile` — the on-disk YAML envelope

```rust
/// The `kind: Snapshot` YAML file written to
/// `/users/<username>/snapshots/<agent>-<timestamp>.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: SnapshotMetadata,
    pub spec: SnapshotSpec,
}

impl SnapshotFile {
    pub fn new(metadata: SnapshotMetadata, spec: SnapshotSpec) -> Self {
        Self {
            api_version: "avix/v1".into(),
            kind: "Snapshot".into(),
            metadata,
            spec,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, AvixError> {
        serde_yaml::from_str(s).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    pub fn to_yaml(&self) -> Result<String, AvixError> {
        serde_yaml::to_string(self).map_err(|e| AvixError::ConfigParse(e.to_string()))
    }

    /// Generate the human-readable snapshot name from agent name and captured_at.
    pub fn make_name(agent_name: &str, captured_at: &chrono::DateTime<chrono::Utc>) -> String {
        format!("{}-{}", agent_name, captured_at.format("%Y%m%d-%H%M"))
    }

    /// VFS path where this snapshot is stored.
    pub fn vfs_path(&self, username: &str) -> String {
        format!("/users/{}/snapshots/{}.yaml", username, self.metadata.name)
    }
}
```

### Updated `SnapshotStore` — add YAML-capable methods

Keep the existing in-memory `HashMap<String, Vec<Snapshot>>` store but:
- Replace `Snapshot` with `SnapshotFile` as the stored type.
- Add `save_yaml()` / `from_yaml()` convenience round-trips.
- Keep `save()`, `load()`, `list()`, `delete()`, `snapshot_count()` with updated types.

```rust
pub struct SnapshotStore {
    // agent_name → Vec<SnapshotFile>
    store: tokio::sync::RwLock<HashMap<String, Vec<SnapshotFile>>>,
}

impl SnapshotStore {
    pub async fn save(&self, snap: SnapshotFile) -> Result<String, SnapshotError> {
        let id = snap.metadata.name.clone();
        let agent = snap.metadata.agent_name.clone();
        self.store.write().await.entry(agent).or_default().push(snap);
        Ok(id)
    }
    pub async fn load(&self, name: &str) -> Result<SnapshotFile, SnapshotError> { ... }
    pub async fn list(&self, agent_name: &str) -> Vec<SnapshotFile> { ... }
    pub async fn delete(&self, name: &str) -> Result<(), SnapshotError> { ... }
    pub async fn snapshot_count(&self, agent_name: &str) -> usize { ... }
}
```

> Note: `SnapshotStore` uses `tokio::sync::RwLock` (not `std::sync::RwLock`) to align with the async runtime.

### Keep `SnapshotMessage` (it's used by capture)

`SnapshotMessage { role: String, content: String }` is correct and moves to `snapshot/capture.rs` but is NOT part of the YAML envelope — it is the raw message history that gets summarised into `spec.contextSummary` + `spec.contextTokenCount`. Keep it as an internal type.

---

## TDD Test Plan

File: `crates/avix-core/src/snapshot/capture.rs` (unit tests under `#[cfg(test)]`)

```rust
// T-SA-01: SnapshotFile round-trips through YAML
#[test]
fn snapshot_file_round_trips() {
    use chrono::Utc;
    let meta = SnapshotMetadata {
        name: "researcher-20260315-0741".into(),
        agent_name: "researcher".into(),
        source_pid: 57,
        captured_at: Utc::now(),
        captured_by: CapturedBy::Kernel,
        trigger: SnapshotTrigger::Auto,
    };
    let spec = SnapshotSpec {
        goal: "Research quantum computing".into(),
        context_summary: "Found 12 sources. Synthesising.".into(),
        context_token_count: 64_000,
        memory: SnapshotMemory { episodic_events: 14, semantic_keys: 8 },
        pending_requests: vec![PendingRequest {
            request_id: "req-abc124".into(),
            resource: "tool".into(),
            name: "web".into(),
            status: "in-flight".into(),
        }],
        pipes: vec![SnapshotPipe { pipe_id: "pipe-001".into(), state: "open".into() }],
        environment: SnapshotEnvironment {
            temperature: 0.7,
            capability_token: "sha256:tokenSig789".into(),
        },
        checksum: "sha256:snap001".into(),
    };
    let file = SnapshotFile::new(meta, spec);
    let yaml = file.to_yaml().unwrap();
    let parsed = SnapshotFile::from_str(&yaml).unwrap();
    assert_eq!(parsed.kind, "Snapshot");
    assert_eq!(parsed.metadata.agent_name, "researcher");
    assert_eq!(parsed.metadata.source_pid, 57);
    assert_eq!(parsed.spec.context_token_count, 64_000);
    assert_eq!(parsed.spec.pending_requests.len(), 1);
    assert_eq!(parsed.spec.pipes.len(), 1);
}

// T-SA-02: CapturedBy serialises and deserialises correctly
#[test]
fn captured_by_round_trips() {
    let cases = [
        (CapturedBy::Kernel, "kernel"),
        (CapturedBy::User(1001), "user:1001"),
        (CapturedBy::Agent(57), "agent:57"),
    ];
    for (variant, expected) in &cases {
        let yaml = serde_yaml::to_string(variant).unwrap();
        assert!(yaml.trim() == *expected, "serialise: got {yaml:?}, want {expected:?}");
        let parsed: CapturedBy = serde_yaml::from_str(expected).unwrap();
        assert_eq!(parsed, *variant);
    }
}

// T-SA-03: SnapshotTrigger serialises to lowercase
#[test]
fn snapshot_trigger_serialises_lowercase() {
    assert_eq!(serde_yaml::to_string(&SnapshotTrigger::Sigsave).unwrap().trim(), "sigsave");
    assert_eq!(serde_yaml::to_string(&SnapshotTrigger::Auto).unwrap().trim(), "auto");
}

// T-SA-04: vfs_path() generates correct path
#[test]
fn snapshot_file_vfs_path() {
    let file = make_test_snapshot("researcher", 57);
    let path = file.vfs_path("alice");
    assert!(path.starts_with("/users/alice/snapshots/researcher-"), "got: {path}");
    assert!(path.ends_with(".yaml"), "got: {path}");
}

// T-SA-05: SnapshotStore save + load + list (async)
#[tokio::test]
async fn snapshot_store_save_load_list() {
    let store = SnapshotStore::new();
    let snap = make_test_snapshot("researcher", 42);
    let name = store.save(snap.clone()).await.unwrap();
    let loaded = store.load(&name).await.unwrap();
    assert_eq!(loaded.metadata.source_pid, 42);
    let list = store.list("researcher").await;
    assert_eq!(list.len(), 1);
}

// T-SA-06: SnapshotStore delete
#[tokio::test]
async fn snapshot_store_delete() {
    let store = SnapshotStore::new();
    let name = store.save(make_test_snapshot("researcher", 42)).await.unwrap();
    store.delete(&name).await.unwrap();
    assert_eq!(store.snapshot_count("researcher").await, 0);
}

// T-SA-07: make_name() produces readable format
#[test]
fn snapshot_make_name_format() {
    use chrono::TimeZone;
    let dt = chrono::Utc.with_ymd_and_hms(2026, 3, 15, 7, 41, 0).unwrap();
    assert_eq!(SnapshotFile::make_name("researcher", &dt), "researcher-20260315-0741");
}
```

---

## Implementation Notes

- Keep existing `SnapshotStore` API shape; change stored type from `Snapshot` to `SnapshotFile` and switch to `tokio::sync::RwLock`.
- `SnapshotMessage` stays as an internal type in `capture.rs`; it is **not** part of the on-disk YAML. The capture logic (Gap B) will convert `Vec<SnapshotMessage>` into `context_summary` and `context_token_count`.
- `CapturedBy` needs manual `Serialize`/`Deserialize` — implement via `serde::Serializer::serialize_str` and `Deserializer::deserialize_str` with split-on-colon parsing.
- The `checksum` field will be populated by the capture logic (Gap B). In Gap A, the tests may use a placeholder string. The field is required (not `Option`) so tests must supply one.
- Rename `snap_` → `snap` module and `snap_.rs` file if desired — the trailing underscore was a workaround for the `snap` keyword in older Rust. It is no longer needed as `snap` is not reserved.

---

## Success Criteria

- [ ] `SnapshotFile` round-trips through YAML (T-SA-01)
- [ ] `CapturedBy` serialises as `"kernel"`, `"user:<uid>"`, `"agent:<pid>"` (T-SA-02)
- [ ] `SnapshotTrigger` serialises lowercase (T-SA-03)
- [ ] `vfs_path()` produces correct VFS path (T-SA-04)
- [ ] `SnapshotStore` async save/load/list (T-SA-05)
- [ ] `SnapshotStore` delete (T-SA-06)
- [ ] `make_name()` format (T-SA-07)
- [ ] `cargo clippy --workspace -- -D warnings` passes
