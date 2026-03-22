# Param Gap D — Resolved Config at Agent Spawn

> **Status:** Not started
> **Priority:** High — makes `/proc/<pid>/resolved.yaml` authoritative instead of decorative
> **Depends on:** Gap B (typed structs), Gap C (resolution engine)
> **Affects:** `avix-core/src/executor/runtime_executor.rs`, new `avix-core/src/params/resolved_file.rs`

---

## Problem

`RuntimeExecutor::write_proc_files()` writes `/proc/<pid>/resolved.yaml` with
hard-coded literal values (`contextWindowTokens: 64000`, `maxToolChainLength: 50`).
It does not call the resolution engine, does not read user or crew defaults, and does not
clamp against limits. The file is structurally present but semantically wrong.

The spec (`docs/spec/param-resolved.md`) requires:

- `resolved.yaml` is produced by merging system defaults → crew defaults → user defaults
  → manifest overrides, clamped against effective limits.
- It is the **authoritative** config the agent runs under — `RuntimeExecutor` must read
  it back and enforce its values (e.g. `max_tool_chain`, `timeout_sec`).
- It is immutable for the lifetime of the agent (no rewrite on limit changes).

---

## What Needs to Be Built

### 1. `ResolvedFile` envelope type (`params/resolved_file.rs`)

The VFS file format wrapping `ResolvedConfig` and optional `Annotations`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFile {
    pub api_version: String,         // "avix/v1"
    pub kind: String,                // "Resolved"
    pub metadata: ResolvedMetadata,
    pub resolved: ResolvedConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedMetadata {
    pub target: String,              // "agent-manifest"
    pub resolved_at: String,         // ISO8601
    pub resolved_for: ResolvedFor,
    pub crews: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFor {
    pub username: String,
    pub pid: Option<u32>,           // None in preview files
}

impl ResolvedFile {
    pub fn from_str(s: &str) -> Result<Self, AvixError>;
    pub fn to_yaml(&self) -> Result<String, AvixError>;

    /// Build from resolution engine output.
    pub fn new(
        username: String,
        pid: Option<u32>,
        crews: Vec<String>,
        resolved: ResolvedConfig,
        annotations: Option<Annotations>,
    ) -> Self;
}
```

### 2. Wire resolution engine into `RuntimeExecutor::spawn_with_registry`

In `executor/runtime_executor.rs`, replace the hard-coded resolved.yaml generation with:

```rust
async fn write_resolved_file(
    &self,
    pid: Pid,
    agent_manifest: &AgentManifest,
    username: &str,
    crews: &[String],
) -> Result<(), AvixError> {
    // Load resolver inputs from VFS
    let loader = ResolverInputLoader::new(&self.vfs);
    let mut input = loader.load(username, crews).await?;

    // Apply manifest overrides as the top-priority layer
    input.manifest = AgentDefaults::from_manifest(agent_manifest);

    // Run resolution engine
    let (resolved_config, _annotations) = ParamResolver::resolve(&input)?;

    // Build file (no annotations in /proc/<pid>/resolved.yaml by default)
    let file = ResolvedFile::new(
        username.to_string(),
        Some(pid.as_u32()),
        crews.to_vec(),
        resolved_config,
        None,
    );

    let yaml = file.to_yaml()?;
    self.vfs.write(&format!("/proc/{}/resolved.yaml", pid), yaml.as_bytes()).await?;
    Ok(())
}
```

`AgentDefaults::from_manifest` extracts fields from `AgentManifest.spec` into the
defaults layer format so the resolver can treat them uniformly.

### 3. Enforce resolved values in the agent turn loop

After writing resolved.yaml at spawn, read it back and propagate to the agent's runtime
settings:

```rust
// In RuntimeExecutor, after write_proc_files:
let resolved_file = self.read_resolved_file(pid).await?;
let cfg = &resolved_file.resolved;

// Set enforced values:
self.max_tool_chain = cfg.entrypoint.max_tool_chain;
self.timeout_sec    = cfg.environment.timeout_sec;
self.model          = cfg.entrypoint.model_preference.clone();
```

These replace whatever hard-coded defaults existed before.

### 4. Preview file at user-level (`/proc/users/<u>/resolved/agent-manifest.yaml`)

Written whenever limits or user defaults change (wired to a `SIGUSR1` handler in a later
gap). For now, write it at agent spawn as a side-effect:

```rust
async fn write_user_preview_file(
    &self,
    username: &str,
    crews: &[String],
    resolved: &ResolvedConfig,
    annotations: Annotations,
) -> Result<(), AvixError> {
    let file = ResolvedFile::new(
        username.to_string(),
        None,   // no pid in preview
        crews.to_vec(),
        resolved.clone(),
        Some(annotations),   // preview includes full annotation block
    );
    let path = format!("/proc/users/{}/resolved/agent-manifest.yaml", username);
    let yaml = file.to_yaml()?;
    self.vfs.write(&path, yaml.as_bytes()).await?;
    Ok(())
}
```

### 5. `ResolveError` spawn failure path

If `ParamResolver::resolve` returns `ResolutionError::HardViolation`, the spawn must be
rejected and an error record written to `/proc/spawn-errors/<request-id>.yaml`:

```yaml
apiVersion: avix/v1
kind: ResolveError
metadata:
  requestId: <uuid>
  timestamp: <ISO8601>
  username: alice
error:
  code: HardViolation
  field: entrypoint.modelPreference
  requestedValue: claude-opus-4
  allowedValues: [claude-sonnet-4, claude-haiku-4]
  constrainedBy: /kernel/limits/agent-manifest.yaml
```

---

## TDD Test Plan

Tests go in `crates/avix-core/tests/resolved_at_spawn.rs` (new file).

```rust
// T-D-01: resolved.yaml contains values from system defaults when no overrides
#[tokio::test]
async fn spawn_writes_resolved_yaml_from_system_defaults() {
    let (executor, vfs) = setup_executor_with_defaults(/* max_tool_chain: 5 */).await;
    let pid = executor.spawn(make_manifest(), "alice", &[]).await.unwrap();
    let raw = vfs.read(&format!("/proc/{}/resolved.yaml", pid)).await.unwrap();
    let file = ResolvedFile::from_str(&String::from_utf8(raw).unwrap()).unwrap();
    assert_eq!(file.resolved.entrypoint.max_tool_chain, 5);
    assert_eq!(file.metadata.resolved_for.pid, Some(pid.as_u32()));
}

// T-D-02: Manifest overrides appear in resolved.yaml
#[tokio::test]
async fn spawn_resolved_reflects_manifest_override() {
    let (executor, vfs) = setup_executor_with_defaults(/* max_tool_chain: 5 */).await;
    let mut manifest = make_manifest();
    manifest.spec.max_tool_chain = Some(8);  // manifest requests 8
    let pid = executor.spawn(manifest, "alice", &[]).await.unwrap();
    let file = read_resolved(&vfs, pid).await;
    assert_eq!(file.resolved.entrypoint.max_tool_chain, 8);
}

// T-D-03: System limits clamp manifest value
#[tokio::test]
async fn spawn_resolved_clamped_by_system_limits() {
    let (executor, vfs) = setup_executor_with_limits(/* max_tool_chain max: 10 */).await;
    let mut manifest = make_manifest();
    manifest.spec.max_tool_chain = Some(20);
    let pid = executor.spawn(manifest, "alice", &[]).await.unwrap();
    let file = read_resolved(&vfs, pid).await;
    assert_eq!(file.resolved.entrypoint.max_tool_chain, 10);
}

// T-D-04: User defaults override system defaults in resolved.yaml
#[tokio::test]
async fn spawn_resolved_includes_user_defaults() {
    let (executor, vfs) = setup_executor_with_user_defaults(
        "alice", /* max_tool_chain: 7 */
    ).await;
    let pid = executor.spawn(make_manifest(), "alice", &[]).await.unwrap();
    let file = read_resolved(&vfs, pid).await;
    assert_eq!(file.resolved.entrypoint.max_tool_chain, 7);
}

// T-D-05: resolved.yaml is immutable — rewriting blocked
#[tokio::test]
async fn resolved_yaml_is_not_rewritten_on_second_call() {
    let (executor, vfs) = setup_executor_with_defaults().await;
    let pid = executor.spawn(make_manifest(), "alice", &[]).await.unwrap();
    let before = read_resolved(&vfs, pid).await;
    // Simulate a limits change (SIGUSR1 scenario)
    change_system_limits(&vfs, /* max_tool_chain max: 3 */).await;
    // resolved.yaml for this pid should NOT change
    let after = read_resolved(&vfs, pid).await;
    assert_eq!(before.resolved.entrypoint.max_tool_chain, after.resolved.entrypoint.max_tool_chain);
}

// T-D-06: HardViolation spawns write to /proc/spawn-errors/
#[tokio::test]
async fn spawn_hard_violation_writes_error_file() {
    let (executor, vfs) = setup_executor_with_limits(/* model: only sonnet/haiku */).await;
    let mut manifest = make_manifest();
    manifest.spec.model_preference = Some("claude-opus-4".into());
    let result = executor.spawn(manifest, "alice", &[]).await;
    assert!(result.is_err());
    // spawn-errors dir should have one entry
    let errors = vfs.list("/proc/spawn-errors/").await.unwrap();
    assert_eq!(errors.len(), 1);
}

// T-D-07: RuntimeExecutor enforces max_tool_chain from resolved.yaml
#[tokio::test]
async fn executor_enforces_max_tool_chain_from_resolved() {
    let (executor, vfs) = setup_executor_with_limits(/* max_tool_chain max: 3 */).await;
    let pid = executor.spawn(make_manifest(), "alice", &[]).await.unwrap();
    // drive the agent loop to make more than 3 tool calls
    let result = run_agent_exceeding_tool_chain(&executor, pid, /* tool_calls: 5 */).await;
    assert!(matches!(result, Err(AvixError::ToolChainLimitExceeded)));
}

// T-D-08: Preview file written at /proc/users/<u>/resolved/
#[tokio::test]
async fn spawn_writes_user_preview_file() {
    let (executor, vfs) = setup_executor_with_defaults().await;
    executor.spawn(make_manifest(), "alice", &[]).await.unwrap();
    let path = "/proc/users/alice/resolved/agent-manifest.yaml";
    assert!(vfs.exists(path).await.unwrap());
    let file = ResolvedFile::from_str(
        &String::from_utf8(vfs.read(path).await.unwrap()).unwrap()
    ).unwrap();
    // Preview file includes annotations
    assert!(file.annotations.is_some());
}
```

---

## Implementation Notes

- `ResolvedFile` must be fully round-trippable: `to_yaml` → `from_str` → same struct.
- The `annotations` block is omitted from `/proc/<pid>/resolved.yaml` (per spec) but
  included in `/proc/users/<u>/resolved/agent-manifest.yaml`.
- `resolved.yaml` is written atomically — use a temp file + rename, not a direct write,
  to prevent partial reads if the agent is inspected immediately after spawn.
- The file is written before the agent's first LLM call. If writing fails, spawn aborts.
- `AgentDefaults::from_manifest` only sets fields that are `Some(_)` in the manifest —
  `None` means "use whatever lower layers provide", not "set to None".

---

## Success Criteria

- [ ] `resolved.yaml` reflects merged values (not hard-coded literals) at spawn (T-D-01 to T-D-04)
- [ ] System limits clamp manifest overrides and are reflected in resolved.yaml (T-D-03)
- [ ] `resolved.yaml` is not rewritten when limits change post-spawn (T-D-05)
- [ ] Spawn failures write a `ResolveError` to `/proc/spawn-errors/` (T-D-06)
- [ ] `RuntimeExecutor` enforces `max_tool_chain` from resolved config (T-D-07)
- [ ] Preview file written at `/proc/users/<u>/resolved/` with annotations (T-D-08)
- [ ] All T-D-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
