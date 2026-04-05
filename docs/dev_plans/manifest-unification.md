# Manifest Unification

> Unify agent and service manifest formats into one consistent `manifest.yaml` with a
> shared envelope and kind-specific `spec`. Two kinds: `Agent` and `Service`.

---

## Problem

There are currently three inconsistent manifest formats — plus the actual
`agents/universal-tool-explorer/manifest.yaml` uses a fourth:

| Format | File | `apiVersion` | `kind` | Used by |
|---|---|---|---|---|
| `AgentManifestFile` | `agent_manifest/manifest_file.rs` | no | no | `PackageValidator` |
| `AgentManifest` | `agent_manifest/schema.rs` | yes | `AgentManifest` | runtime loader |
| `ServiceUnit` | `service/yaml.rs` | no | no | packaging + runtime |
| actual UTE file | `agents/universal-tool-explorer/manifest.yaml` | yes | `AgentManifest` | nothing (unparseable by any struct) |

Additionally `PackageBuilder::read_name` for services reads a TOML file named
`service.unit` — a file that doesn't exist anywhere in the repo.

---

## Goal

- One file name: always `manifest.yaml`
- One envelope: `apiVersion`, `kind`, `metadata` (required), `packaging` (optional)
- Two kinds: `Agent` and `Service`
- Packaging detects type by reading `kind`, not by checking which file exists
- The installed `manifest.yaml` is loadable by the kernel without modification

---

## New Canonical Format

### Agent

```yaml
apiVersion: avix/v1
kind: Agent

metadata:
  name: universal-tool-explorer
  version: "0.1.0"
  description: "Discovers and exercises every available tool in the Avix OS."
  author: "Avix Core Team"
  license: MIT                       # optional
  tags: [demo, tools, explorer]      # optional
  createdAt: "2026-04-05T10:00:00Z"  # optional

packaging:                           # optional section; omit in dev
  source: "github:avix/agents"
  signature: "sha256:abc123def456"   # "sha256:" skips verification

spec:
  systemPromptPath: system-prompt.md   # path relative to package root
  requestedCapabilities:
    - kernel:*
    - fs:*
    - workspace:*
    - llm:inference
    - proc:*
  entrypoint:
    type: llm-loop
    modelRequirements:
      minContextWindow: 128000
      requiredCapabilities: [tool_use]
      recommended: claude-sonnet-4
    maxToolChain: 50
    maxTurnsPerGoal: 50
  tools:
    required: []
    optional: []
  memory:
    workingContext: dynamic
    episodicPersistence: true
    semanticStoreAccess: read-only
  snapshot:
    mode: per-turn
    restoreOnCrash: true
    compressionEnabled: true
  defaults:
    goalTemplate: "Explore the Avix tool registry..."
    environment:
      temperature: 0.7
      topP: 0.9
      timeoutSec: 300
  visibility: public   # optional
  scope: system        # optional
```

### Service

```yaml
apiVersion: avix/v1
kind: Service

metadata:
  name: workspace
  version: "0.1.0"
  description: "High-level workspace abstraction with session history integration"
  author: "avix/workspace"

packaging:
  source: "system"
  signature: "sha256:"   # skip verification for built-in services

spec:
  binary: /services/workspace/bin/workspace
  language: rust
  restart: always
  restartDelay: 5s
  maxConcurrent: 20
  queueMax: 100
  queueTimeout: 5s
  runAs: service
  requires: []
  after:
    - memfs.svc
    - router.svc
  capabilities:
    callerScoped: true
    required: []
    hostAccess:
      - filesystem
  tools:
    namespace: /tools/workspace/
    provides: []
  jobs:
    maxActive: 3
    jobTimeout: 3600s
    persist: false
```

---

## Rust Struct Design

### Shared types (add to `agent_manifest/schema.rs` or new `manifest/common.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMetadata {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackagingMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
```

### Updated `AgentManifest` (`agent_manifest/schema.rs`)

```rust
pub struct AgentManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,                  // "Agent"
    pub metadata: ManifestMetadata,
    #[serde(default)]
    pub packaging: PackagingMetadata,
    pub spec: AgentSpec,
}

// Rename AgentManifestSpec → AgentSpec; add new fields
pub struct AgentSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_path: Option<String>,        // NEW (was systemPrompt.path)
    #[serde(default)]
    pub requested_capabilities: Vec<String>,       // NEW (was requestedCapabilities)
    #[serde(default)]
    pub entrypoint: ManifestEntrypoint,            // unchanged
    #[serde(default)]
    pub tools: ManifestTools,                      // unchanged
    #[serde(default)]
    pub memory: ManifestMemory,                    // unchanged
    #[serde(default)]
    pub snapshot: ManifestSnapshot,                // unchanged
    #[serde(default)]
    pub defaults: ManifestDefaults,                // remove system_prompt field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,                // NEW
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,                     // NEW
}
```

`ManifestDefaults.system_prompt` (inline string) is **removed** — replaced by
`spec.systemPromptPath`. The runtime reads the prompt file from the VFS at spawn time.

### New `ServiceManifest` (`service/yaml.rs`)

```rust
pub struct ServiceManifest {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,                  // "Service"
    pub metadata: ManifestMetadata,
    #[serde(default)]
    pub packaging: PackagingMetadata,
    pub spec: ServiceSpec,
}

#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    // merged from ServiceSection
    pub binary: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default = "default_restart_delay")]
    pub restart_delay: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: u32,
    #[serde(default = "default_queue_max")]
    pub queue_max: u32,
    #[serde(default = "default_queue_timeout")]
    pub queue_timeout: String,
    #[serde(default)]
    pub run_as: RunAs,
    // merged from UnitSection
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
    // sub-sections unchanged
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    pub tools: ToolsSection,
    #[serde(default)]
    pub jobs: JobsSection,
}

impl ServiceUnit {
    /// Convert from the new on-disk format to the internal runtime struct.
    pub fn from_manifest(m: &ServiceManifest) -> Self { ... }
}
```

### Unified parser (`AnyManifest`)

```rust
pub enum AnyManifest {
    Agent(AgentManifest),
    Service(ServiceManifest),
}

impl AnyManifest {
    pub fn from_yaml(s: &str) -> Result<Self, AvixError>;
    pub fn from_file(path: &Path) -> Result<Self, AvixError>;
    pub fn metadata(&self) -> &ManifestMetadata;
    pub fn packaging(&self) -> &PackagingMetadata;
}
```

`from_yaml` does a two-step parse: first parse `{ kind: String }` to branch, then
parse the full document into `AgentManifest` or `ServiceManifest`.

---

## File-by-File Changes

### `crates/avix-core/src/agent_manifest/schema.rs`
- Add `ManifestMetadata`, `PackagingMetadata`
- Replace `AgentManifestMetadata` with `ManifestMetadata`
- Add `packaging: PackagingMetadata` field to `AgentManifest`
- Rename `AgentManifestSpec` → `AgentSpec`; add `system_prompt_path`, `requested_capabilities`, `visibility`, `scope`
- Remove `ManifestDefaults.system_prompt`
- Update `kind` in tests: `"AgentManifest"` → `"Agent"`
- Update MINIMAL_YAML, FULL_YAML test constants and all assertions

### `crates/avix-core/src/agent_manifest/manifest_file.rs`
- **DELETE** — `AgentManifestFile` is replaced by `AgentManifest`
- Remove from `agent_manifest/mod.rs` exports

### `crates/avix-core/src/service/yaml.rs`
- Add `ServiceManifest`, `ServiceSpec` structs
- Add `ServiceManifest::load(path: &Path) -> Result<Self, AvixError>`
- Add `ServiceUnit::from_manifest(m: &ServiceManifest) -> Self`
- Update `ServiceUnit::load` to try `manifest.yaml` (kind=Service) first, fall back to
  old `service.yaml` flat format for backward compat during transition
- Update `load_for_service` to scan for `manifest.yaml` in `<name>@*/` dirs
- Update all in-file tests to write the new format

### `crates/avix-core/src/service/installer.rs` (line 72)
```rust
// Before
let unit = ServiceUnit::load(&tmp_dir.path().join("service.yaml"))?;
// After
let unit = ServiceManifest::load(&tmp_dir.path().join("manifest.yaml"))
    .map(|m| ServiceUnit::from_manifest(&m))?;
```

### `crates/avix-core/src/service/lifecycle.rs` (line 284)
```rust
// Before
let unit_path = entry.path().join("service.yaml");
// After
let unit_path = entry.path().join("manifest.yaml");
// and load via ServiceManifest::load then ServiceUnit::from_manifest
```

### `crates/avix-core/src/packaging/mod.rs`
```rust
// New PackageType::detect — reads kind field instead of checking file presence
pub fn detect(dir: &Path) -> Result<Self, AvixError> {
    let path = dir.join("manifest.yaml");
    let content = std::fs::read_to_string(&path)
        .map_err(|_| AvixError::ConfigParse("manifest.yaml not found".into()))?;
    #[derive(serde::Deserialize)]
    struct KindProbe { kind: String }
    let probe: KindProbe = serde_yaml::from_str(&content)
        .map_err(|e| AvixError::ConfigParse(format!("manifest.yaml parse error: {e}")))?;
    match probe.kind.as_str() {
        "Agent"   => Ok(Self::Agent),
        "Service" => Ok(Self::Service),
        other     => Err(AvixError::ConfigParse(format!("unknown kind: {other}"))),
    }
}
```
Update tests accordingly.

### `crates/avix-core/src/packaging/validator.rs`
- `validate_agent`: parse `manifest.yaml` as `AgentManifest`; check `metadata.name`, `metadata.version`, `spec.system_prompt_path` file exists
- `validate_service`: parse `manifest.yaml` as `ServiceManifest`; check `metadata.name`, `metadata.version`, `spec.binary`, `bin/` dir non-empty
- Remove `AgentManifestFile` import
- Update all tests to write new-format YAML

### `crates/avix-core/src/packaging/builder.rs`
- `read_name`: use `AnyManifest::from_file(dir.join("manifest.yaml"))` for both kinds; return `m.metadata().name.clone()`
- Delete the broken TOML `service.unit` branch
- Update test fixtures to use new-format `manifest.yaml`

### `crates/avix-core/src/packaging/scaffold.rs`
- `scaffold_agent`: emit new Agent envelope format
- `scaffold_service`: emit new Service envelope format (writes `manifest.yaml`, not `service.yaml`)
- Update tests

### `agents/universal-tool-explorer/manifest.yaml`
| Old field | New location |
|---|---|
| `kind: AgentManifest` | `kind: Agent` |
| `metadata.tags` | `metadata.tags` (unchanged) |
| `spec.contextWindowTokens` | `spec.entrypoint.modelRequirements.minContextWindow` |
| `spec.maxToolChainLength` | `spec.entrypoint.maxToolChain` |
| `spec.hilRequiredTools` | remove (not in new spec) |
| `spec.requestedCapabilities` | `spec.requestedCapabilities` (unchanged) |
| `spec.systemPrompt.path` | `spec.systemPromptPath` |
| `spec.defaultGoalTemplate` | `spec.defaults.goalTemplate` |
| `spec.visibility` | `spec.visibility` (unchanged) |
| `spec.scope` | `spec.scope` (unchanged) |
Add `packaging:` section with dev defaults (`signature: "sha256:"`).

### `services/workspace/service.yaml` → `services/workspace/manifest.yaml`
Delete `service.yaml`. Create `manifest.yaml` with `kind: Service` mapping:
- `name` / `version` → `metadata.name` / `metadata.version`
- `unit.description` / `unit.author` → `metadata.description` / `metadata.author`
- `unit.after` → `spec.after`
- all `service.*` → `spec.*` (camelCase)
- `capabilities.*` → `spec.capabilities.*`
- `tools.*` → `spec.tools.*`
Add `packaging: { source: system, signature: "sha256:" }`.

### `services/mcp-bridge/service.yaml` → `services/mcp-bridge/manifest.yaml`
Same conversion as workspace above.

### `docs/architecture/15-packaging.md`
- Replace the flat `manifest.yaml` schema block with the Agent reference manifest above
- Replace the `service.yaml` schema block with the Service reference manifest above
- Update "Package Detection" section: detection reads `kind` field, not file presence
- Update "Validation" section: required fields are now `metadata.name`, `metadata.version`
- Remove references to `service.yaml` as a packaging artifact

---

## Design Decisions

1. **`kind` values are `Agent` and `Service`** — not `AgentManifest`/`ServiceManifest`. Short and
   consistent with Kubernetes style.

2. **`packaging` section is optional** — defaults to all-`None`. Omitting it is valid for
   dev/local packages. `signature: "sha256:"` (empty hash) skips GPG check (existing behavior).

3. **`ServiceUnit` is kept as internal runtime state** — `ServiceManifest` is the on-disk format;
   `ServiceUnit::from_manifest` converts for the runtime. This avoids churning
   `ServiceProcess`, `ServiceWatchdog`, `ServiceRegistry`.

4. **`AgentManifestFile` is deleted** — it was a separate flat-format struct that duplicated
   `AgentManifest` with different field names. The single `AgentManifest` now covers both
   packaging and runtime.

5. **`ManifestDefaults.system_prompt` (inline string) is removed** — replaced by
   `spec.systemPromptPath` pointing to a file. Inline prompts don't work for packaging (you
   can't embed a multi-line file inline without quoting). The runtime reads the file via VFS
   at spawn time.

6. **Backward compat shim for `service.yaml`** — `ServiceUnit::load` tries `manifest.yaml`
   first; falls back to legacy flat format. Removes cleanly once all callers are updated.
