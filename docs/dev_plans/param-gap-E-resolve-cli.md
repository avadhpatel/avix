# Param Gap E — avix resolve CLI + avix config reload

> **Status:** Not started
> **Priority:** Medium — operator tooling; does not block agent execution
> **Depends on:** Gap B (types), Gap C (resolution engine), Gap D (ResolvedFile)
> **Affects:** `avix-cli/src/main.rs`, new `avix-core/src/cli/resolve.rs`, new `avix-core/src/cli/config_reload.rs`

---

## Problem

Two CLI commands described in the spec are completely absent:

1. **`avix resolve`** (`docs/spec/param-resolved.md §CLI Triage Tool`) — lets admins and
   users inspect the full resolution trace for a given user/agent combination without
   actually spawning an agent. Essential for debugging permission and limit problems.

2. **`avix config reload`** (`docs/spec/kernel-config.md §Reload Behaviour`) — applies
   changes to the live kernel for fields that do not require a restart. Currently the
   only way to apply any config change is a full restart.

---

## What Needs to Be Built

### 1. `avix resolve` command

#### CLI surface (`avix-cli/src/main.rs`)

```
avix resolve <kind> --user <username> [--agent <name>] [--explain] [--limits-only] [--crew <crew>] [--dry-run]
```

Options:
| Flag | Meaning |
|------|---------|
| `<kind>` | Currently always `agent-manifest` |
| `--user <u>` | Whose defaults and limits to load |
| `--agent <name>` | Agent manifest name from `/services/` or `/users/<u>/agents/` (optional) |
| `--explain` | Include full annotations block in output |
| `--limits-only` | Show effective limits only (no defaults merging, no resolved) |
| `--crew <crew>` | Simulate adding this crew membership for the resolution (dry-run) |
| `--dry-run` | Print what would happen without writing any file |

Add to the existing `Cmd` enum:

```rust
enum Cmd {
    Config { sub: ConfigCmd },
    Run { ... },
    Resolve(ResolveArgs),
}

#[derive(Parser)]
pub struct ResolveArgs {
    pub kind: String,
    #[arg(long)]
    pub user: String,
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long)]
    pub explain: bool,
    #[arg(long)]
    pub limits_only: bool,
    #[arg(long, name = "crew")]
    pub extra_crew: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub root: PathBuf,
}
```

#### Core logic (`avix-core/src/cli/resolve.rs`)

```rust
pub struct ResolveParams {
    pub root: PathBuf,
    pub kind: String,
    pub username: String,
    pub agent_name: Option<String>,
    pub explain: bool,
    pub limits_only: bool,
    pub extra_crew: Option<String>,
    pub dry_run: bool,
}

pub struct ResolveResult {
    pub output: String,   // YAML string written to stdout
}

pub async fn run_resolve(params: ResolveParams) -> Result<ResolveResult, AvixError> {
    // 1. Open VFS at root
    // 2. Load user record from /etc/avix/users.yaml → get crew memberships
    // 3. If --crew, add that crew to the membership list for simulation
    // 4. Load agent manifest from /services/<name>/manifest.yaml or /users/<u>/agents/<name>/manifest.yaml (if --agent)
    // 5. If --limits-only: compute effective limits, serialize, return
    // 6. Run ResolverInputLoader::load(username, crews)
    // 7. If agent manifest loaded, apply as manifest overrides
    // 8. Run ParamResolver::resolve(input)
    // 9. Build ResolvedFile { annotations: if explain then Some(annotations) else None }
    // 10. Serialize to YAML, return
}
```

Output format: a `Resolved` YAML document written to stdout (matches spec).

#### Examples

```sh
# Show resolved config for alice spawning a researcher agent
avix resolve agent-manifest --user alice --agent researcher --root ~/avix-data

# Show with full annotation (provenance of every field)
avix resolve agent-manifest --user alice --agent researcher --explain --root ~/avix-data

# Show what limits are in effect for alice across all crews
avix resolve agent-manifest --user alice --explain --limits-only --root ~/avix-data

# Simulate what would happen if alice joined the automation crew
avix resolve agent-manifest --user alice --crew automation --dry-run --root ~/avix-data
```

---

### 2. `avix config reload` command

#### CLI surface

```
avix config reload [--check] [--root <root>]
```

Options:
| Flag | Meaning |
|------|---------|
| `--check` | Validate the new config files but do not apply them |
| `--root <root>` | Root data directory |

Add to `ConfigCmd`:

```rust
enum ConfigCmd {
    Init { root, user, role, credential_type, mode },
    Reload(ReloadArgs),
}

#[derive(Parser)]
pub struct ReloadArgs {
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub root: PathBuf,
}
```

#### Core logic (`avix-core/src/cli/config_reload.rs`)

```rust
pub struct ReloadParams {
    pub root: PathBuf,
    pub check_only: bool,
}

pub struct ReloadResult {
    pub reloaded_sections: Vec<String>,   // sections that were applied
    pub restart_required: Vec<String>,     // sections that changed but need restart
    pub errors: Vec<String>,              // validation errors if any
}

pub async fn run_config_reload(params: ReloadParams) -> Result<ReloadResult, AvixError> {
    // 1. Read current kernel.yaml from disk
    // 2. Parse into KernelConfig, call validate()
    // 3. Read previously-loaded KernelConfig from a runtime state file
    //    (or compare against compiled-in baseline if no runtime state)
    // 4. Call KernelConfig::requires_restart(current, new) per section
    // 5. If --check: return validation result without applying
    // 6. Otherwise: emit a reload signal / update in-memory config for hot-reload sections
    //    Note: actual kernel process signaling is out of scope for this gap —
    //    write a marker file at /run/avix/reload-pending that the kernel polls,
    //    or send SIGHUP to the kernel PID if a PID file exists at /run/avix/kernel.pid
    // 7. Return ReloadResult listing which sections reloaded vs require restart
}
```

**Restart-required sections** (from spec reload table):
- `ipc`
- `models.kernel`
- `secrets.masterKey`
- `secrets.store`

All other sections are hot-reloadable.

---

## TDD Test Plan

### resolve tests — `crates/avix-core/tests/cli_resolve.rs`

```rust
// T-E-01: avix resolve returns valid Resolved YAML
#[tokio::test]
async fn resolve_returns_valid_yaml() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        root: dir.path().to_path_buf(),
        kind: "agent-manifest".into(),
        username: "alice".into(),
        agent_name: None,
        explain: false,
        limits_only: false,
        extra_crew: None,
        dry_run: false,
    }).await.unwrap();
    let parsed: ResolvedFile = ResolvedFile::from_str(&result.output).unwrap();
    assert_eq!(parsed.metadata.resolved_for.username, "alice");
}

// T-E-02: --explain includes annotations block
#[tokio::test]
async fn resolve_explain_includes_annotations() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        root: dir.path().to_path_buf(),
        explain: true,
        ..base_resolve_params()
    }).await.unwrap();
    let parsed: ResolvedFile = ResolvedFile::from_str(&result.output).unwrap();
    assert!(parsed.annotations.is_some());
    assert!(!parsed.annotations.unwrap().is_empty());
}

// T-E-03: --limits-only returns only effective limits YAML
#[tokio::test]
async fn resolve_limits_only_returns_limits() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        root: dir.path().to_path_buf(),
        limits_only: true,
        ..base_resolve_params()
    }).await.unwrap();
    // Output should be LimitsFile, not ResolvedFile
    assert!(result.output.contains("kind: Limits"));
}

// T-E-04: --crew simulates additional crew membership
#[tokio::test]
async fn resolve_dry_run_crew_simulation() {
    let dir = setup_avix_root_with_alice_and_automation_crew().await;
    let result = run_resolve(ResolveParams {
        root: dir.path().to_path_buf(),
        extra_crew: Some("automation".into()),
        dry_run: true,
        ..base_resolve_params()
    }).await.unwrap();
    let parsed = ResolvedFile::from_str(&result.output).unwrap();
    // automation crew has tighter limits — effective max_tool_chain should be lower
    assert!(parsed.resolved.entrypoint.max_tool_chain <= 5);
}

// T-E-05: Unknown user returns error
#[tokio::test]
async fn resolve_unknown_user_returns_error() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_resolve(ResolveParams {
        root: dir.path().to_path_buf(),
        username: "nonexistent".into(),
        ..base_resolve_params()
    }).await;
    assert!(result.is_err());
}
```

### config reload tests — `crates/avix-core/tests/cli_config_reload.rs`

```rust
// T-E-06: reload --check validates config and reports sections
#[tokio::test]
async fn config_reload_check_reports_sections() {
    let dir = setup_avix_root_with_alice().await;
    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    }).await.unwrap();
    // All hot-reload sections should be listed as reloadable
    assert!(result.reloaded_sections.contains(&"scheduler".to_string()));
    assert!(result.reloaded_sections.contains(&"observability".to_string()));
    assert!(result.restart_required.is_empty());  // nothing changed
}

// T-E-07: reload --check reports restart-required when ipc changed
#[tokio::test]
async fn config_reload_check_detects_ipc_change() {
    let dir = setup_avix_root_with_alice().await;
    // Modify ipc.timeoutMs in kernel.yaml
    mutate_kernel_yaml_ipc_timeout(&dir, 9999).await;
    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    }).await.unwrap();
    assert!(result.restart_required.contains(&"ipc".to_string()));
}

// T-E-08: reload --check fails on invalid kernel.yaml
#[tokio::test]
async fn config_reload_check_fails_on_invalid_config() {
    let dir = setup_avix_root_with_alice().await;
    // Write invalid kernel.yaml (tick_ms = 0)
    write_invalid_kernel_yaml(&dir).await;
    let result = run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: true,
    }).await;
    assert!(result.is_err());
}

// T-E-09: reload writes reload-pending marker file
#[tokio::test]
async fn config_reload_writes_marker_file() {
    let dir = setup_avix_root_with_alice().await;
    run_config_reload(ReloadParams {
        root: dir.path().to_path_buf(),
        check_only: false,
    }).await.unwrap();
    // Marker file should exist (kernel polls it)
    assert!(dir.path().join("run/avix/reload-pending").exists());
}
```

---

## Implementation Notes

- `run_resolve` is a pure I/O function — open VFS, load inputs, call engine, serialize.
  Keep it thin; all logic lives in `ResolverInputLoader` and `ParamResolver`.
- `run_config_reload` in this gap only handles validation and writing the reload-pending
  marker file. Actual in-process config hot-swapping requires the live kernel and is a
  separate concern.
- The `--dry-run` flag for `avix resolve` does not write any files. It only affects
  whether the per-user preview file at `/proc/users/<u>/resolved/` is updated. When
  `dry_run = true`, skip the VFS write.
- `avix resolve` exits 0 on success; exits 1 on resolution error (HardViolation); exits
  2 on I/O error or missing config.
- `avix config reload --check` exits 0 if config is valid and all changed sections are
  hot-reloadable; exits 1 if restart is required; exits 2 on validation error.

---

## Success Criteria

- [ ] `avix resolve agent-manifest --user alice` emits valid Resolved YAML (T-E-01)
- [ ] `--explain` includes annotations block (T-E-02)
- [ ] `--limits-only` emits Limits YAML (T-E-03)
- [ ] `--crew` simulates additional crew membership (T-E-04)
- [ ] `avix config reload --check` validates and classifies sections (T-E-06, T-E-07)
- [ ] `avix config reload --check` fails on invalid config (T-E-08)
- [ ] `avix config reload` writes reload-pending marker (T-E-09)
- [ ] All T-E-* tests pass
- [ ] `cargo clippy --workspace -- -D warnings` passes
