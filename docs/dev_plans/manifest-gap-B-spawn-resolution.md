# Manifest Gap B — Manifest Loading, Signature Verification, and Spawn-Time Resolution

> **Status:** Not started
> **Priority:** High — agents cannot be spawned from manifests without this
> **Depends on:** manifest-gap-A-schema
> **Affects:** `avix-core/src/agent_manifest/loader.rs` (new), `avix-core/src/agent_manifest/resolver.rs` (new), `avix-core/src/executor/spawn.rs`

---

## Problem

Even after Gap A defines the `AgentManifest` struct, nothing in the system:

1. Loads a manifest from the VFS at spawn time
2. Verifies its signature (SHA-256 integrity check)
3. Validates the model requirements against the selected model
4. Resolves the tool grant — intersects `spec.tools.required` + `spec.tools.optional` with the
   user's permitted tools from `/etc/avix/users.yaml`
5. Fails spawn with a clear error when a required tool is denied
6. Injects the manifest's `systemPrompt` and rendered `goalTemplate` into the LLM context

`SpawnParams` currently carries only a bare `agent_name` string. There is no path from
"user runs `avix spawn researcher --goal foo`" to "correct system prompt + validated tool list
appear in the executor".

---

## Spec Summary

```
Spawn-time resolution order:
  1. Load /bin/<agent>/manifest.yaml (or /users/<u>/bin/<agent>/manifest.yaml)
  2. Verify sha256 signature matches manifest hash
  3. Select model: --model arg OR KernelConfig.models.default; validate against modelRequirements
  4. Load user ACL from /etc/avix/users.yaml
  5. Grant tools = intersection(manifest.required ∪ optional, user_permitted_tools)
  6. Any required tool missing from grant → reject spawn with ManifestError::RequiredToolDenied
  7. Any optional tool missing → silently omit from granted_tools
  8. Always grant built-in kernel tools (cap/list, cap/request-tool, job/watch, cap/escalate)
  9. Render goalTemplate: replace {{key}} with spawn --var key=value args
 10. Inject systemPrompt + rendered goal into RuntimeExecutor context
```

---

## What Needs to Be Built

### 1. `ManifestError` (extend `AvixError` or add dedicated variant)

Add to `crates/avix-core/src/error.rs`:

```rust
// New variants under AvixError:
ManifestNotFound { path: String },
ManifestSignatureMismatch { path: String },
ManifestKindMismatch { expected: String, found: String },
RequiredToolDenied { tool: String, agent: String },
ModelRequirementsNotMet { reason: String },
```

### 2. `ManifestLoader` in `agent_manifest/loader.rs`

Loads and validates an `AgentManifest` from the VFS.

```rust
pub struct ManifestLoader {
    vfs: Arc<VfsRouter>,
}

impl ManifestLoader {
    pub fn new(vfs: Arc<VfsRouter>) -> Self { Self { vfs } }

    /// Load a manifest for a named agent.
    ///
    /// Resolution order:
    ///   1. `/bin/<name>/manifest.yaml`  (system-installed, symlink → versioned dir)
    ///   2. `/users/<username>/bin/<name>/manifest.yaml`  (user-installed)
    ///
    /// Returns `ManifestNotFound` if neither path exists.
    pub async fn load(&self, name: &str, username: &str) -> Result<AgentManifest, AvixError>;

    /// Load from an exact VFS path. Used internally and for tests.
    pub async fn load_from_path(&self, path: &str) -> Result<AgentManifest, AvixError>;

    /// Verify the manifest's `metadata.signature` against a SHA-256 hash of its
    /// canonical YAML content (the raw bytes read from VFS, before parsing).
    ///
    /// The `signature` field itself is excluded from the hash computation —
    /// it is zeroed/removed from the bytes before hashing.
    pub fn verify_signature(raw_yaml: &[u8], manifest: &AgentManifest) -> Result<(), AvixError>;
}
```

**Signature verification approach:**

The spec stores `metadata.signature: "sha256:<hex>"`. At load time:
1. Read raw YAML bytes from VFS.
2. Parse into `AgentManifest`.
3. Re-serialise the manifest with the `signature` field set to `""` (empty sentinel).
4. Compute `sha256(canonical_bytes)`.
5. Compare to `metadata.signature` prefix-stripped of `"sha256:"`.

For v1, if the signature is the empty string (`"sha256:"`), skip verification (dev/test
manifests). A missing or malformed signature always fails.

> **Note:** In production, manifests are signed at package build time. The kernel does not
> mint signatures — it only verifies them. Tools for package build are out of scope for v1
> runtime development.

### 3. `ToolGrantResolver` in `agent_manifest/resolver.rs`

Computes the final `granted_tools: Vec<String>` from manifest + user ACL.

```rust
/// Represents a user's tool permission set loaded from /etc/avix/users.yaml.
pub struct UserToolPermissions {
    pub allowed_tools: Vec<String>,   // from crew + additionalTools
    pub denied_tools: Vec<String>,    // from crew + user-level deniedTools
}

pub struct ToolGrantResolver;

impl ToolGrantResolver {
    /// Compute the granted tool list.
    ///
    /// Algorithm:
    ///   permitted = (crew.allowedTools + user.additionalTools) - user.deniedTools
    ///   granted = intersection(manifest.required ∪ manifest.optional, permitted)
    ///
    /// Returns Err(RequiredToolDenied) if any tool in manifest.required is absent from granted.
    pub fn resolve(
        manifest_tools: &ManifestTools,
        user_permissions: &UserToolPermissions,
    ) -> Result<Vec<String>, AvixError>;

    /// Built-in kernel tools always granted regardless of ACL.
    /// Per spec and ADR-04, these are Category 2 tools registered at spawn by RuntimeExecutor.
    /// The resolver adds them to the granted list unconditionally.
    pub const ALWAYS_GRANTED: &'static [&'static str] = &[
        "cap/request-tool",
        "cap/escalate",
        "cap/list",
        "job/watch",
    ];
}
```

### 4. `ModelValidator` in `agent_manifest/resolver.rs`

```rust
pub struct ModelValidator;

impl ModelValidator {
    /// Validate that `selected_model` satisfies `requirements`.
    ///
    /// For v1, validation checks:
    ///   - model name is non-empty
    ///   - (future) context window lookup via llm.svc — stubbed in v1
    ///
    /// Returns Err(ModelRequirementsNotMet) with a descriptive message.
    pub fn validate(
        selected_model: &str,
        requirements: &ModelRequirements,
    ) -> Result<(), AvixError>;
}
```

> **v1 scope note:** Full model capability lookup (querying `llm.svc` for actual context window)
> is deferred. In v1, `validate()` checks the model name is non-empty and logs a warning if
> `requirements.min_context_window > 0`. Full enforcement lands when `llm.svc` exposes a model
> metadata endpoint.

### 5. `GoalRenderer` in `agent_manifest/resolver.rs`

```rust
pub struct GoalRenderer;

impl GoalRenderer {
    /// Render `goal_template` by substituting `{{key}}` patterns with values from `vars`.
    ///
    /// Variables are passed as `key=value` pairs from `avix spawn --var key=value`.
    /// Unknown `{{key}}` tokens that have no corresponding var are left as-is.
    pub fn render(template: &str, vars: &HashMap<String, String>) -> String;
}
```

### 6. `ResolvedSpawnContext` — output of full spawn resolution

```rust
/// The fully resolved context ready to hand to RuntimeExecutor.
pub struct ResolvedSpawnContext {
    pub manifest: AgentManifest,
    pub selected_model: String,
    pub granted_tools: Vec<String>,     // manifest intersection ∩ user ACL + built-ins
    pub system_prompt: String,          // manifest.defaults.systemPrompt (required at spawn)
    pub rendered_goal: String,          // goalTemplate with {{vars}} substituted, or raw goal
}
```

### 7. `SpawnResolver` — orchestrates all steps

```rust
pub struct SpawnResolver {
    loader: ManifestLoader,
}

impl SpawnResolver {
    pub fn new(vfs: Arc<VfsRouter>) -> Self;

    /// Full spawn-time resolution pipeline.
    ///
    ///   load manifest → verify signature → validate model
    ///   → resolve tool grant → render goal → return ResolvedSpawnContext
    pub async fn resolve(
        &self,
        agent_name: &str,
        username: &str,
        goal: &str,
        selected_model: &str,
        vars: HashMap<String, String>,
        user_permissions: &UserToolPermissions,
    ) -> Result<ResolvedSpawnContext, AvixError>;
}
```

### 8. Extend `SpawnParams` in `executor/spawn.rs`

Add the resolved context to `SpawnParams` so `RuntimeExecutor::new` can consume it:

```rust
pub struct SpawnParams {
    pub pid: Pid,
    pub agent_name: String,
    pub goal: String,
    pub spawned_by: String,
    pub token: CapabilityToken,
    pub session_id: String,
    // ── new fields from manifest resolution ──────────────────────────────────
    pub system_prompt: Option<String>,       // from manifest.defaults.systemPrompt
    pub selected_model: String,              // resolved model name
}
```

---

## TDD Test Plan

File: `crates/avix-core/src/agent_manifest/resolver.rs` under `#[cfg(test)]`
File: `crates/avix-core/src/agent_manifest/loader.rs` under `#[cfg(test)]`

```rust
// T-MGB-01: ToolGrantResolver grants intersection of required + optional vs permitted
#[test]
fn resolver_grants_intersection() {
    let manifest_tools = ManifestTools {
        required: vec!["fs/read".into(), "web/search".into()],
        optional: vec!["code/interpreter".into()],
    };
    let user = UserToolPermissions {
        allowed_tools: vec!["fs/read".into(), "web/search".into(), "web/fetch".into()],
        denied_tools: vec![],
    };
    let granted = ToolGrantResolver::resolve(&manifest_tools, &user).unwrap();
    assert!(granted.contains(&"fs/read".to_string()));
    assert!(granted.contains(&"web/search".to_string()));
    // code/interpreter is optional and not in user perms — silently absent
    assert!(!granted.contains(&"code/interpreter".to_string()));
    // built-ins always present
    assert!(granted.contains(&"cap/list".to_string()));
}

// T-MGB-02: Required tool denied → Err(RequiredToolDenied)
#[test]
fn resolver_fails_when_required_tool_denied() {
    let manifest_tools = ManifestTools {
        required: vec!["fs/read".into(), "bash".into()],
        optional: vec![],
    };
    let user = UserToolPermissions {
        allowed_tools: vec!["fs/read".into()],
        denied_tools: vec![],
    };
    let result = ToolGrantResolver::resolve(&manifest_tools, &user);
    assert!(matches!(
        result,
        Err(AvixError::RequiredToolDenied { tool, .. }) if tool == "bash"
    ));
}

// T-MGB-03: Built-ins always in granted list regardless of ACL
#[test]
fn resolver_always_grants_built_ins() {
    let manifest_tools = ManifestTools { required: vec![], optional: vec![] };
    let user = UserToolPermissions { allowed_tools: vec![], denied_tools: vec![] };
    let granted = ToolGrantResolver::resolve(&manifest_tools, &user).unwrap();
    for builtin in ToolGrantResolver::ALWAYS_GRANTED {
        assert!(granted.contains(&builtin.to_string()), "missing builtin: {}", builtin);
    }
}

// T-MGB-04: User denied_tools override allowed_tools
#[test]
fn user_denied_tools_override_allowed() {
    let manifest_tools = ManifestTools {
        required: vec![],
        optional: vec!["fs/write".into()],
    };
    let user = UserToolPermissions {
        allowed_tools: vec!["fs/write".into()],
        denied_tools: vec!["fs/write".into()],  // explicitly denied at user level
    };
    let granted = ToolGrantResolver::resolve(&manifest_tools, &user).unwrap();
    assert!(!granted.contains(&"fs/write".to_string()));
}

// T-MGB-05: GoalRenderer substitutes {{key}} with var values
#[test]
fn goal_renderer_substitutes_vars() {
    let vars = HashMap::from([
        ("topic".into(), "quantum computing".into()),
        ("format".into(), "markdown".into()),
    ]);
    let rendered = GoalRenderer::render(
        "Research: {{topic}}. Format: {{format}}.",
        &vars,
    );
    assert_eq!(rendered, "Research: quantum computing. Format: markdown.");
}

// T-MGB-06: GoalRenderer leaves unknown {{key}} tokens as-is
#[test]
fn goal_renderer_leaves_unknown_tokens() {
    let vars = HashMap::new();
    let rendered = GoalRenderer::render("Research: {{topic}}.", &vars);
    assert_eq!(rendered, "Research: {{topic}}.");
}

// T-MGB-07: GoalRenderer with empty template returns template unchanged
#[test]
fn goal_renderer_empty_template() {
    let rendered = GoalRenderer::render("", &HashMap::new());
    assert_eq!(rendered, "");
}

// T-MGB-08: ManifestLoader loads manifest from VFS path
#[tokio::test]
async fn loader_loads_manifest_from_vfs() {
    let vfs = build_test_vfs_with_manifest("researcher", "1.3.0", RESEARCHER_YAML).await;
    let loader = ManifestLoader::new(vfs);
    let manifest = loader.load_from_path("/bin/researcher@1.3.0/manifest.yaml").await.unwrap();
    assert_eq!(manifest.metadata.name, "researcher");
    assert_eq!(manifest.metadata.version, "1.3.0");
}

// T-MGB-09: ManifestLoader returns ManifestNotFound for missing path
#[tokio::test]
async fn loader_returns_not_found_for_missing_manifest() {
    let vfs = build_empty_test_vfs().await;
    let loader = ManifestLoader::new(vfs);
    let result = loader.load_from_path("/bin/nonexistent@1.0.0/manifest.yaml").await;
    assert!(matches!(result, Err(AvixError::ManifestNotFound { .. })));
}

// T-MGB-10: ManifestLoader rejects manifest with wrong kind
#[tokio::test]
async fn loader_rejects_wrong_kind() {
    let wrong_kind_yaml = r#"
apiVersion: avix/v1
kind: SomethingElse
metadata:
  name: x
  version: 1.0.0
  description: x
  author: x
  createdAt: 2026-01-01T00:00:00Z
  signature: "sha256:"
spec: {}
"#;
    let vfs = build_test_vfs_with_raw("/bin/x@1.0.0/manifest.yaml", wrong_kind_yaml).await;
    let loader = ManifestLoader::new(vfs);
    let result = loader.load_from_path("/bin/x@1.0.0/manifest.yaml").await;
    assert!(matches!(result, Err(AvixError::ManifestKindMismatch { .. })));
}

// T-MGB-11: SpawnResolver returns ResolvedSpawnContext with correct fields
#[tokio::test]
async fn spawn_resolver_produces_correct_context() {
    let vfs = build_test_vfs_with_manifest("echo-bot", "1.0.0", ECHO_BOT_YAML).await;
    let resolver = SpawnResolver::new(vfs);
    let user_perms = UserToolPermissions {
        allowed_tools: vec!["fs/read".into()],
        denied_tools: vec![],
    };
    let ctx = resolver.resolve(
        "echo-bot", "alice", "echo hello", "claude-sonnet-4",
        HashMap::new(), &user_perms,
    ).await.unwrap();
    assert_eq!(ctx.manifest.metadata.name, "echo-bot");
    assert_eq!(ctx.selected_model, "claude-sonnet-4");
    assert!(ctx.granted_tools.contains(&"cap/list".to_string()));
    assert_eq!(ctx.rendered_goal, "echo hello");
}
```

---

## Implementation Notes

- `ManifestLoader.load()` tries system path first, then user path. It does NOT fall through
  silently — `ManifestNotFound` is returned if neither exists.
- Signature verification: for `metadata.signature == "sha256:"` (dev manifests), skip
  verification and log a `warn!("signature verification skipped for dev manifest: {}")`.
- `ToolGrantResolver::resolve` iterates `required` tools first; the first denied required tool
  triggers an early `Err`. Denied optional tools are simply absent from the output vec.
- `ALWAYS_GRANTED` built-ins are appended at the end after all manifest/ACL intersection.
  Deduplication is not needed in practice but add `.dedup()` for safety.
- `GoalRenderer` is a simple string-replace loop. No regex dependency required:
  iterate over `vars`, call `template.replace(&format!("{{{{{}}}}}", k), v)` for each entry.
- `ModelValidator::validate` in v1 only checks the model string is non-empty. Add a
  `tracing::warn!` if `min_context_window > 0` noting that full validation is deferred.
- Do NOT load `/etc/avix/users.yaml` inside this module. The caller passes in a
  `UserToolPermissions` struct. Parsing users.yaml is the kernel's responsibility at spawn.

---

## Success Criteria

- [ ] Tool grant intersection produces correct `granted_tools` (T-MGB-01)
- [ ] Required tool denied returns `Err(RequiredToolDenied)` (T-MGB-02)
- [ ] Built-ins always present in granted list (T-MGB-03)
- [ ] User `denied_tools` override allowed tools (T-MGB-04)
- [ ] `GoalRenderer` substitutes `{{key}}` placeholders (T-MGB-05)
- [ ] `GoalRenderer` leaves unknown tokens unchanged (T-MGB-06)
- [ ] `ManifestLoader` loads manifest from VFS correctly (T-MGB-08)
- [ ] `ManifestLoader` returns `ManifestNotFound` for missing path (T-MGB-09)
- [ ] `ManifestLoader` rejects wrong `kind` (T-MGB-10)
- [ ] `SpawnResolver` produces a correctly populated `ResolvedSpawnContext` (T-MGB-11)
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --check` passes
