# Agent Status Gap A — Full Schema Alignment & Live VFS Writes

> **Status:** Not started
> **Priority:** High — prerequisite for agent observability, snapshot, and HIL escalation
> **Depends on:** None
> **Affects:**
> - `avix-core/src/process/entry.rs`
> - `avix-core/src/executor/runtime_executor.rs`
> - `avix-core/src/process/table.rs`

---

## Problem

`/proc/<pid>/status.yaml` is written **once at spawn** with a minimal template and is
**never updated** during execution. The spec (`docs/spec/agent-status.md`) requires the
kernel to keep this file live — reflecting the agent's current state, context usage,
tool call counters, pipe connections, signal history, and wall-clock metrics.

Additionally, several spec fields are entirely absent from the current
`ProcessEntry` struct and status.yaml output:

| Spec field                     | Current state                                      |
|--------------------------------|----------------------------------------------------|
| `status.state`                 | Always `running`; `pending` variant missing from enum |
| `status.contextUsed`           | Not tracked or written                            |
| `status.contextLimit`          | Not tracked or written                            |
| `status.toolCallsThisTurn`     | `tool_chain_depth` exists in `ProcessEntry` but is not written to status.yaml |
| `status.lastActivityAt`        | Tracked in `SessionEntry` but absent from status.yaml |
| `status.waitingOn`             | No field exists anywhere                          |
| `status.tools.denied`          | Not tracked or written (only `granted` exists)    |
| `status.pipes`                 | Pipe manager writes `/proc/<pid>/pipes/` files but nothing in status.yaml |
| `status.signals.lastReceived`  | Not tracked                                       |
| `status.signals.pendingCount`  | Not tracked                                       |
| `status.metrics.tokensConsumed`| Not tracked                                       |
| `status.metrics.toolCallsTotal`| Not tracked (only per-turn depth exists)          |
| `status.metrics.wallTimeSec`   | Not tracked                                       |

---

## What Needs to Be Built

### 1. Align `ProcessStatus` enum — add `Pending`

**File:** `avix-core/src/process/entry.rs`

The spec defines six states: `pending | running | paused | waiting | stopped | crashed`.
`Pending` is missing from the current enum.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Pending,
    Running,
    Paused,
    Waiting,
    Stopped,
    Crashed,
}
```

---

### 2. Add `WaitingOn` enum

**File:** `avix-core/src/process/entry.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaitingOn {
    HumanApproval,
    PipeRead,
    PipeWrite,
    Signal,
}
```

---

### 3. Extend `ProcessEntry` with all missing fields

**File:** `avix-core/src/process/entry.rs`

Add the following fields to `ProcessEntry`:

```rust
pub struct ProcessEntry {
    // --- existing fields (keep) ---
    pub pid: u32,
    pub name: String,
    pub kind: ProcessKind,
    pub status: ProcessStatus,
    pub parent: Option<u32>,
    pub spawned_by_user: String,
    pub granted_tools: Vec<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
    pub tool_chain_depth: u32,

    // --- new fields ---
    pub goal: String,
    pub spawned_at: DateTime<Utc>,

    // context tracking
    pub context_used: u64,
    pub context_limit: u64,

    // activity
    pub last_activity_at: DateTime<Utc>,
    pub waiting_on: Option<WaitingOn>,

    // denied tools
    pub denied_tools: Vec<String>,

    // signal tracking
    pub last_signal_received: Option<String>,
    pub pending_signal_count: u32,

    // lifetime metrics
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    pub wall_time_sec: u64,   // derived: (Utc::now() - spawned_at).num_seconds()
}
```

---

### 4. Add update methods to `ProcessTable`

**File:** `avix-core/src/process/table.rs`

```rust
impl ProcessTable {
    /// Called by RuntimeExecutor after each LLM response with the token count returned.
    pub async fn record_tokens(&self, pid: u32, tokens: u64) -> Result<(), ProcessError>;

    /// Called when the agent's state changes (e.g. SIGPAUSE received).
    pub async fn set_state(&self, pid: u32, status: ProcessStatus, waiting_on: Option<WaitingOn>) -> Result<(), ProcessError>;

    /// Called after each tool call completes.
    pub async fn increment_tool_calls_total(&self, pid: u32) -> Result<(), ProcessError>;

    /// Called by RuntimeExecutor each turn with the current context window token count.
    pub async fn update_context(&self, pid: u32, used: u64) -> Result<(), ProcessError>;

    /// Called when a signal is delivered to the process.
    pub async fn record_signal(&self, pid: u32, signal_name: &str) -> Result<(), ProcessError>;

    /// Called after each tool call with updated last_activity_at.
    pub async fn touch_activity(&self, pid: u32) -> Result<(), ProcessError>;
}
```

---

### 5. `AgentStatusFile` — serialisable snapshot struct

**File:** `avix-core/src/process/status_file.rs` (new)

This struct mirrors the YAML schema and is serialised to write `/proc/<pid>/status.yaml`.
It is constructed from a `ProcessEntry` on every write.

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusFile {
    pub api_version: String,   // "avix/v1"
    pub kind: String,          // "AgentStatus"
    pub metadata: AgentStatusMetadata,
    pub status: AgentStatusSpec,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusMetadata {
    pub name: String,
    pub pid: u32,
    pub spawned_at: DateTime<Utc>,
    pub spawned_by: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSpec {
    pub state: ProcessStatus,
    pub goal: String,
    pub context_used: u64,
    pub context_limit: u64,
    pub tool_calls_this_turn: u32,
    pub last_activity_at: DateTime<Utc>,
    pub waiting_on: Option<WaitingOn>,
    pub tools: AgentStatusTools,
    pub pipes: Vec<AgentStatusPipe>,
    pub signals: AgentStatusSignals,
    pub metrics: AgentStatusMetrics,
}

#[derive(Debug, Serialize)]
pub struct AgentStatusTools {
    pub granted: Vec<String>,
    pub denied: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusPipe {
    pub id: String,
    pub target_pid: u32,
    pub direction: String,  // "in" | "out"
    pub state: String,      // "open" | "closed" | "draining"
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusSignals {
    pub last_received: Option<String>,
    pub pending_count: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusMetrics {
    pub tokens_consumed: u64,
    pub tool_calls_total: u32,
    pub wall_time_sec: u64,
}
```

**Constructor:**

```rust
impl AgentStatusFile {
    pub fn from_entry(entry: &ProcessEntry, pipes: Vec<AgentStatusPipe>) -> Self { ... }
}
```

`wall_time_sec` is computed at construction time: `(Utc::now() - entry.spawned_at).num_seconds() as u64`.

---

### 6. Live status.yaml writes in `RuntimeExecutor`

**File:** `avix-core/src/executor/runtime_executor.rs`

Replace the single spawn-time write with a helper `write_status_yaml` called at:

| Event                          | Fields updated                                              |
|-------------------------------|-------------------------------------------------------------|
| Agent spawn (`Pending` → `Running`) | All fields initialised                              |
| After each LLM turn           | `contextUsed`, `toolCallsThisTurn`, `lastActivityAt`, `metrics` |
| After each tool call          | `toolCallsThisTurn`, `toolCallsTotal`, `lastActivityAt`    |
| State change (SIGPAUSE etc.)  | `state`, `waitingOn`                                       |
| Signal received               | `signals.lastReceived`, `signals.pendingCount`             |
| Agent exit                    | `state: stopped` or `state: crashed`                      |

```rust
async fn write_status_yaml(&self, pid: u32) -> Result<()> {
    let entry = self.process_table.get(pid).await?;
    let pipes = self.pipe_manager.pipes_for_pid(pid).await?;
    let file = AgentStatusFile::from_entry(&entry, pipes);
    let yaml = serde_yaml::to_string(&file)?;
    let path = format!("/proc/{}/status.yaml", pid);
    self.vfs.write(&path, yaml.into_bytes()).await?;
    Ok(())
}
```

---

## Test Plan

All tests live in `crates/avix-core/src/process/` under `#[cfg(test)]` or in
`crates/avix-core/tests/`.

### Unit Tests

```rust
// entry.rs
#[test]
fn process_status_serializes_all_six_variants() {
    for (variant, expected) in [
        (ProcessStatus::Pending, "pending"),
        (ProcessStatus::Running, "running"),
        (ProcessStatus::Paused, "paused"),
        (ProcessStatus::Waiting, "waiting"),
        (ProcessStatus::Stopped, "stopped"),
        (ProcessStatus::Crashed, "crashed"),
    ] {
        let yaml = serde_yaml::to_string(&variant).unwrap();
        assert!(yaml.trim() == expected, "got {yaml}");
    }
}

#[test]
fn waiting_on_serializes_kebab_case() {
    let s = serde_yaml::to_string(&WaitingOn::HumanApproval).unwrap();
    assert!(s.trim() == "human-approval");
}
```

```rust
// status_file.rs
#[test]
fn agent_status_file_round_trips_yaml() {
    let entry = ProcessEntry {
        pid: 42,
        name: "test-agent".into(),
        // ... fill required fields
        spawned_at: Utc::now(),
        status: ProcessStatus::Running,
        tool_chain_depth: 2,
        tokens_consumed: 1000,
        tool_calls_total: 5,
        context_used: 5000,
        context_limit: 200_000,
        ..Default::default()
    };
    let file = AgentStatusFile::from_entry(&entry, vec![]);
    let yaml = serde_yaml::to_string(&file).unwrap();
    assert!(yaml.contains("kind: AgentStatus"));
    assert!(yaml.contains("state: running"));
    assert!(yaml.contains("contextUsed: 5000"));
    assert!(yaml.contains("tokensConsumed: 1000"));
    assert!(yaml.contains("wallTimeSec:"));
}
```

### Integration Test

```rust
// tests/agent_status_vfs.rs
#[tokio::test]
async fn status_yaml_updated_after_tool_call() {
    // spawn a test agent with a mocked LLM that makes one tool call
    // assert /proc/<pid>/status.yaml exists after spawn with state: running
    // trigger a tool call
    // re-read /proc/<pid>/status.yaml
    // assert toolCallsTotal == 1, lastActivityAt is recent
}

#[tokio::test]
async fn status_yaml_reflects_paused_state() {
    // spawn agent, deliver SIGPAUSE
    // read /proc/<pid>/status.yaml
    // assert state == paused, waitingOn == null
}
```

---

## Success Criteria

- [ ] `ProcessStatus` has all six variants; serialises to lowercase strings.
- [ ] `WaitingOn` serialises to kebab-case strings.
- [ ] `ProcessEntry` contains all spec fields (denied tools, signals, metrics, context).
- [ ] `AgentStatusFile` serialises to YAML matching the spec schema exactly.
- [ ] `write_status_yaml` is called at every lifecycle event listed in the table above.
- [ ] `/proc/<pid>/status.yaml` on a running agent reflects the current turn's tool call count.
- [ ] `state: paused` appears in status.yaml after SIGPAUSE is delivered.
- [ ] `wallTimeSec` is non-zero and increases between reads.
- [ ] `cargo test --workspace` passes, `cargo clippy -- -D warnings` is clean.
