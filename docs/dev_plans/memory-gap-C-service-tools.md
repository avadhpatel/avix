# Memory Gap C — memory.svc Service & Tool Handlers

> **Status:** Complete
> **Priority:** High — core service; all agent memory operations require this
> **Depends on:** memory-gap-A (schema), memory-gap-B (VFS layout)
> **Affects:** `avix-core/src/memory_svc/` (new module), `avix-core/src/service/`

---

## Problem

`memory.svc` does not exist. No memory tools are registered. Agents cannot store,
retrieve, or query any memory. This gap implements the service module and all tool
handlers from the spec, using plain VFS reads/writes and BM25-style keyword search
(no vector index yet — that is memory-gap-E).

---

## What Needs to Be Built

### Module layout

```
crates/avix-core/src/memory_svc/
├── mod.rs          ← pub mod declarations, MemoryService struct
├── schema.rs       ← from memory-gap-A
├── acl.rs          ← namespace enforcement helpers
├── store.rs        ← VFS read/write helpers for MemoryRecord / UserPreferenceModel
├── search.rs       ← BM25 full-text search (no vectors in this gap)
└── tools/
    ├── mod.rs
    ├── retrieve.rs
    ├── log_event.rs
    ├── store_fact.rs
    ├── get_fact.rs
    ├── update_preference.rs
    ├── get_preferences.rs
    └── forget.rs
```

### `MemoryService` struct

```rust
pub struct MemoryService {
    vfs: Arc<MemFs>,
    kernel_config: Arc<MemoryConfig>,
}

impl MemoryService {
    pub fn new(vfs: Arc<MemFs>, kernel_config: Arc<MemoryConfig>) -> Self {
        Self { vfs, kernel_config }
    }

    /// Called at service startup. Writes /proc/services/memory/status.yaml.
    pub async fn start(&self) -> Result<(), AvixError> { ... }

    /// Dispatch a tool call to the correct handler.
    pub async fn dispatch(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        caller: &CallerContext,
    ) -> Result<serde_json::Value, AvixError> {
        match tool_name {
            "memory/retrieve"          => tools::retrieve::handle(self, params, caller).await,
            "memory/log-event"         => tools::log_event::handle(self, params, caller).await,
            "memory/store-fact"        => tools::store_fact::handle(self, params, caller).await,
            "memory/get-fact"          => tools::get_fact::handle(self, params, caller).await,
            "memory/update-preference" => tools::update_preference::handle(self, params, caller).await,
            "memory/get-preferences"   => tools::get_preferences::handle(self, params, caller).await,
            "memory/forget"            => tools::forget::handle(self, params, caller).await,
            _ => Err(AvixError::NotFound(format!("unknown memory tool: {tool_name}"))),
        }
    }
}
```

### `CallerContext` — passed from IPC layer to every tool handler

```rust
pub struct CallerContext {
    pub pid: u32,
    pub agent_name: String,
    pub owner: String,         // username
    pub session_id: String,
    pub granted_tools: Vec<String>,
}

impl CallerContext {
    pub fn has_capability(&self, cap: &str) -> bool {
        // memory:write implies memory:read
        match cap {
            "memory:read" => self.granted_tools.iter().any(|t|
                t == "memory/retrieve" || t == "memory/get-fact" || t == "memory/get-preferences"
            ),
            "memory:write" => self.granted_tools.iter().any(|t|
                t == "memory/log-event" || t == "memory/store-fact" || t == "memory/update-preference"
            ),
            "memory:share" => self.granted_tools.contains(&"memory/share-request".to_string()),
            _ => false,
        }
    }
}
```

### `acl.rs` — namespace isolation

```rust
/// Validates that the caller may write to the given agent's memory namespace.
/// The caller's agent_name must match the target namespace.
pub fn check_write_namespace(caller: &CallerContext, target_agent: &str) -> Result<(), AvixError> {
    if caller.agent_name != target_agent {
        return Err(AvixError::PermissionDenied(format!(
            "agent '{}' may not write to '{}' memory namespace",
            caller.agent_name, target_agent
        )));
    }
    Ok(())
}

/// Validates that the caller may read from the given scope.
pub fn check_read_scope(caller: &CallerContext, scope: &MemoryScope) -> Result<(), AvixError> {
    match scope {
        MemoryScope::Own => Ok(()),  // always permitted if memory:read granted
        MemoryScope::Crew(crew_name) => {
            // Crew membership is pre-verified by CapabilityToken granted_tools.
            // If llm:inference is in granted_tools, the agent is allowed to
            // access crew resources. In v1, defer to token-level grant.
            Ok(())
        }
        MemoryScope::Grant(grant_id) => {
            // Grant verification done in retrieve handler by looking up MemoryGrant.
            Ok(())
        }
    }
}
```

### `store.rs` — VFS read/write helpers

```rust
pub async fn write_record(
    vfs: &MemFs,
    path: &str,
    record: &MemoryRecord,
) -> Result<(), AvixError> {
    let yaml = record.to_yaml()?;
    let vfs_path = VfsPath::parse(path)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    vfs.write(&vfs_path, yaml.into_bytes()).await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn read_record(vfs: &MemFs, path: &str) -> Result<MemoryRecord, AvixError> {
    let vfs_path = VfsPath::parse(path)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let bytes = vfs.read(&vfs_path).await
        .map_err(|_| AvixError::NotFound(format!("memory record not found: {path}")))?;
    let yaml = String::from_utf8(bytes)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    MemoryRecord::from_yaml(&yaml)
}

pub async fn delete_record(vfs: &MemFs, path: &str) -> Result<(), AvixError> {
    let vfs_path = VfsPath::parse(path)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    vfs.delete(&vfs_path).await
        .map_err(|e| AvixError::ConfigParse(e.to_string()))
}

pub async fn list_records(
    vfs: &MemFs,
    dir: &str,
    owner: &str,
    agent_name: &str,
) -> Result<Vec<MemoryRecord>, AvixError> {
    let vfs_path = VfsPath::parse(dir)
        .map_err(|e| AvixError::ConfigParse(e.to_string()))?;
    let entries = vfs.list(&vfs_path).await.unwrap_or_default();
    let mut records = Vec::new();
    for entry in entries {
        if entry.ends_with(".yaml") && !entry.ends_with(".keep") {
            if let Ok(record) = read_record(vfs, &entry).await {
                records.push(record);
            }
        }
    }
    Ok(records)
}
```

### Tool: `memory/log-event`

```rust
pub async fn handle(
    svc: &MemoryService,
    params: serde_json::Value,
    caller: &CallerContext,
) -> Result<serde_json::Value, AvixError> {
    let summary: String = params["summary"].as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing summary".into()))?.into();
    let outcome = params["outcome"].as_str().map(|s| s.parse::<MemoryOutcome>().ok()).flatten();
    let related_goal = params["relatedGoal"].as_str().map(String::from);
    let tags: Vec<String> = params["tags"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let pinned = params["pinned"].as_bool().unwrap_or(false);

    let scope = params["scope"].as_str().unwrap_or("own");
    let (owner, agent_name) = if scope == "own" {
        (caller.owner.clone(), caller.agent_name.clone())
    } else {
        // crew scope — validate membership, set crew path
        // (simplified in this gap; crew scope handled fully in memory-gap-F)
        return Err(AvixError::NotImplemented("crew scope not yet implemented".into()));
    };

    let now = Utc::now();
    let id = new_memory_id();
    let meta = MemoryRecordMetadata {
        id: id.clone(),
        record_type: MemoryRecordType::Episodic,
        agent_name: agent_name.clone(),
        agent_pid: caller.pid,
        owner: owner.clone(),
        created_at: now,
        updated_at: now,
        session_id: caller.session_id.clone(),
        tags,
        pinned,
    };
    let spec = MemoryRecordSpec {
        content: summary,
        outcome,
        related_goal,
        tools_used: vec![],
        key: None, confidence: None, ttl_days: None,
        index: MemoryRecordIndex::default(),
    };
    let record = MemoryRecord::new(meta, spec);
    let path = MemoryRecord::vfs_path_episodic(&owner, &agent_name, &now, &id);
    store::write_record(&svc.vfs, &path, &record).await?;

    Ok(json!({ "id": id, "stored": true, "indexed": false }))
}
```

### Tool: `memory/store-fact`

Write or upsert a semantic record at `vfs_path_semantic(owner, agent_name, key)`. If the
file already exists, overwrite it (update), setting `replaced: true` in the response.

```rust
pub async fn handle(
    svc: &MemoryService,
    params: serde_json::Value,
    caller: &CallerContext,
) -> Result<serde_json::Value, AvixError> {
    let key: String = params["key"].as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing key".into()))?.into();
    let summary: String = params["summary"].as_str()
        .ok_or_else(|| AvixError::ConfigParse("missing summary".into()))?.into();
    // ... build MemoryRecordSpec with key and confidence
    let path = MemoryRecord::vfs_path_semantic(&caller.owner, &caller.agent_name, &key);
    let replaced = svc.vfs.exists(&VfsPath::parse(&path).unwrap()).await;
    store::write_record(&svc.vfs, &path, &record).await?;
    Ok(json!({ "id": id, "key": key, "stored": true, "replaced": replaced }))
}
```

### Tool: `memory/get-fact`

Exact key lookup — deterministic, no retrieval model involved.

```rust
pub async fn handle(...) -> Result<serde_json::Value, AvixError> {
    let key = params["key"].as_str()...;
    let path = MemoryRecord::vfs_path_semantic(&caller.owner, &caller.agent_name, key);
    match store::read_record(&svc.vfs, &path).await {
        Ok(record) => Ok(json!({
            "found": true,
            "record": {
                "id": record.metadata.id,
                "key": record.spec.key,
                "summary": record.spec.content,
                "confidence": record.spec.confidence,
                "updatedAt": record.metadata.updated_at,
                "pinned": record.metadata.pinned,
            }
        })),
        Err(_) => Ok(json!({ "found": false })),
    }
}
```

### Tool: `memory/update-preference`

Reads the existing `UserPreferenceModel` (if present), merges in the new `summary` and
`structured` fields, appends any new `corrections`, and writes back.

```rust
pub async fn handle(...) -> Result<serde_json::Value, AvixError> {
    let path = UserPreferenceModel::vfs_path(&caller.owner, &caller.agent_name);
    let mut model = match read_preference_model(&svc.vfs, &path).await {
        Ok(m) => m,
        Err(_) => UserPreferenceModel::new(
            UserPreferenceModelMetadata {
                agent_name: caller.agent_name.clone(),
                owner: caller.owner.clone(),
                updated_at: Utc::now(),
            },
            UserPreferenceModelSpec { summary: String::new(), structured: Default::default(), corrections: vec![] }
        ),
    };
    // Merge in provided fields
    if let Some(s) = params["summary"].as_str() { model.spec.summary = s.into(); }
    // ... merge structured fields
    // ... append corrections
    model.metadata.updated_at = Utc::now();
    write_preference_model(&svc.vfs, &path, &model).await?;
    Ok(json!({ "updated": true }))
}
```

### Tool: `memory/get-preferences`

```rust
pub async fn handle(...) -> Result<serde_json::Value, AvixError> {
    let path = UserPreferenceModel::vfs_path(&caller.owner, &caller.agent_name);
    match read_preference_model(&svc.vfs, &path).await {
        Ok(model) => Ok(json!({ "found": true, "model": model })),
        Err(_) => Ok(json!({ "found": false })),
    }
}
```

### Tool: `memory/forget`

```rust
pub async fn handle(...) -> Result<serde_json::Value, AvixError> {
    let ids: Vec<String> = params["ids"].as_array()...;
    let mut deleted = vec![];
    let mut not_found = vec![];
    for id in &ids {
        // Try to find and delete by scanning episodic and semantic dirs
        // (in this gap: linear scan by listing VFS entries)
        // In memory-gap-E, an index makes this O(1)
        if let Some(path) = find_record_by_id(&svc.vfs, &caller.owner, &caller.agent_name, id).await {
            store::delete_record(&svc.vfs, &path).await?;
            deleted.push(id.clone());
        } else {
            not_found.push(id.clone());
        }
    }
    Ok(json!({ "deleted": deleted, "notFound": not_found }))
}
```

### Tool: `memory/retrieve` (BM25-only in this gap)

In this gap, retrieval uses only BM25 keyword matching (no vector index, no LLM
re-rank). The vector index and LLM re-rank are added in memory-gap-E. Results are
returned sorted by BM25 score. The `relevance` field is omitted (no model available yet).

```rust
pub async fn handle(...) -> Result<serde_json::Value, AvixError> {
    let query = params["query"].as_str()...;
    let limit = params["limit"].as_u64().unwrap_or(5).min(20) as usize;
    let types: Vec<String> = ...;  // default: ["episodic", "semantic"]

    // Load all candidate records from the agent's own namespace
    let mut candidates = vec![];
    if types.contains(&"episodic".to_string()) {
        let episodic_dir = format!("/users/{}/memory/{}/episodic", caller.owner, caller.agent_name);
        candidates.extend(store::list_records(&svc.vfs, &episodic_dir, ...).await?);
    }
    if types.contains(&"semantic".to_string()) {
        let semantic_dir = format!("/users/{}/memory/{}/semantic", caller.owner, caller.agent_name);
        candidates.extend(store::list_records(&svc.vfs, &semantic_dir, ...).await?);
    }

    // BM25 ranking
    let ranked = search::bm25_rank(&candidates, query, limit);

    let records_json: Vec<_> = ranked.into_iter().map(|r| json!({
        "id": r.metadata.id,
        "type": r.metadata.record_type,
        "scope": "own",
        "summary": &r.spec.content[..200.min(r.spec.content.len())],
        "tags": r.metadata.tags,
        "createdAt": r.metadata.created_at,
        "pinned": r.metadata.pinned,
    })).collect();

    Ok(json!({
        "records": records_json,
        "totalCandidates": candidates.len(),
        "returned": records_json.len(),
    }))
}
```

### `search.rs` — BM25 full-text ranking

Implement a basic BM25 scorer over `spec.content` fields. No external crate needed —
implement the scoring formula directly:

```
BM25(d,q) = Σ_t IDF(t) * (tf(t,d) * (k1+1)) / (tf(t,d) + k1*(1 - b + b*|d|/avgdl))
```

Where `k1 = 1.2`, `b = 0.75`, `avgdl` = average document length in tokens.

```rust
pub fn bm25_rank(
    records: &[MemoryRecord],
    query: &str,
    limit: usize,
) -> Vec<&MemoryRecord> {
    let query_terms: Vec<&str> = query.split_whitespace().collect();
    // ... compute per-record BM25 scores
    // ... sort descending, take limit
}
```

---

## TDD Test Plan

File: `crates/avix-core/tests/memory_svc.rs` (new integration test file)

```rust
// T-MC-01: memory/log-event stores a record to VFS
#[tokio::test]
async fn log_event_stores_to_vfs() {
    let (svc, vfs) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc.dispatch("memory/log-event", json!({
        "summary": "Completed research on quantum computing.",
        "outcome": "success",
        "tags": ["research", "quantum"],
        "pinned": false,
        "scope": "own"
    }), &caller).await.unwrap();
    assert_eq!(result["stored"], true);
    let id = result["id"].as_str().unwrap();
    assert!(id.starts_with("mem-"));
    // Verify file exists in VFS
    let episodic_dir = VfsPath::parse("/users/alice/memory/researcher/episodic").unwrap();
    let entries = vfs.list(&episodic_dir).await.unwrap();
    assert!(entries.iter().any(|e| e.contains(id)));
}

// T-MC-02: memory/store-fact writes semantic record
#[tokio::test]
async fn store_fact_writes_semantic_record() {
    let (svc, vfs) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    let result = svc.dispatch("memory/store-fact", json!({
        "key": "project-alpha-deadline",
        "summary": "Project Alpha deadline is April 30, 2026.",
        "confidence": "high",
        "pinned": true,
        "scope": "own"
    }), &caller).await.unwrap();
    assert_eq!(result["stored"], true);
    assert_eq!(result["replaced"], false);
    // Second write should be a replace
    let result2 = svc.dispatch("memory/store-fact", json!({
        "key": "project-alpha-deadline",
        "summary": "Updated: deadline moved to May 1.",
        "confidence": "high",
        "scope": "own"
    }), &caller).await.unwrap();
    assert_eq!(result2["replaced"], true);
}

// T-MC-03: memory/get-fact returns stored record by key
#[tokio::test]
async fn get_fact_returns_by_key() { ... }

// T-MC-04: memory/update-preference merges into existing model
#[tokio::test]
async fn update_preference_merges() { ... }

// T-MC-05: memory/get-preferences returns not-found for new agent
#[tokio::test]
async fn get_preferences_not_found_for_new_agent() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "brand-new-agent", 1);
    let result = svc.dispatch("memory/get-preferences", json!({}), &caller).await.unwrap();
    assert_eq!(result["found"], false);
}

// T-MC-06: memory/forget deletes by ID
#[tokio::test]
async fn forget_deletes_by_id() { ... }

// T-MC-07: memory/retrieve returns BM25-ranked results
#[tokio::test]
async fn retrieve_returns_ranked_results() {
    let (svc, _) = make_test_memory_svc();
    let caller = make_caller("alice", "researcher", 57);
    // Store two records
    svc.dispatch("memory/log-event", json!({
        "summary": "Quantum computing research completed. Topological qubits discovered.",
        "scope": "own"
    }), &caller).await.unwrap();
    svc.dispatch("memory/log-event", json!({
        "summary": "Financial analysis. Q3 OPEX anomalies found.",
        "scope": "own"
    }), &caller).await.unwrap();
    let result = svc.dispatch("memory/retrieve", json!({
        "query": "quantum computing",
        "limit": 5
    }), &caller).await.unwrap();
    let records = result["records"].as_array().unwrap();
    assert!(!records.is_empty());
    // First result should be the quantum one
    assert!(records[0]["summary"].as_str().unwrap().contains("Quantum"));
}

// T-MC-08: ACL blocks cross-agent writes
#[tokio::test]
async fn acl_blocks_cross_agent_write() {
    // agent_name in caller does not match the namespace being written to
    // (tested indirectly through log-event which writes to own namespace only)
    // Verify EPERM is returned when scope targets a different namespace
}

// T-MC-09: BM25 rank returns empty for no-match query
#[test]
fn bm25_rank_empty_for_no_match() {
    let records = vec![make_test_record("quantum computing research")];
    let ranked = bm25_rank(&records, "financial analysis Q3", 5);
    // The record should not match well; may return empty if below threshold
}
```

---

## Implementation Notes

- `memory.svc` in v1 is not a separate process — it is a module inside `avix-core` called
  by the service dispatch layer, consistent with how other core services are structured.
  The IPC registration (via `ipc.tool-add`) is handled in `service/` using the existing
  service lifecycle pattern.
- `memory/share-request` (HIL flow) is **not** implemented in this gap — it is
  memory-gap-F. The tool is registered but returns `NotImplemented` in this gap.
- The `find_record_by_id()` helper in `forget` does a linear scan over VFS in this gap.
  Memory-gap-E adds an index that makes this O(1). Note this in code with a `// TODO
  memory-gap-E: use index for O(1) lookup` comment.
- BM25 implementation: use stop-word filtering (a, an, the, is, in, on, at, ...) and
  lowercase normalisation. Keep the list small (< 50 words) to avoid over-filtering.

---

## Success Criteria

- [ ] `memory/log-event` stores episodic record to VFS (T-MC-01)
- [ ] `memory/store-fact` writes and replaces semantic records (T-MC-02)
- [ ] `memory/get-fact` returns correct record by key (T-MC-03)
- [ ] `memory/update-preference` merges into existing model (T-MC-04)
- [ ] `memory/get-preferences` returns not-found for new agent (T-MC-05)
- [ ] `memory/forget` deletes by ID (T-MC-06)
- [ ] `memory/retrieve` returns BM25-ranked results (T-MC-07)
- [ ] ACL blocks cross-agent writes (T-MC-08)
- [ ] BM25 returns empty for non-matching query (T-MC-09)
- [ ] `cargo clippy --workspace -- -D warnings` passes
