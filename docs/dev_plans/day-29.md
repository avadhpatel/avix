# Day 29 — Benchmarks: All Performance Targets

> **Goal:** Write and run benchmarks for every declared performance target. All must pass. Fix any implementations that miss targets before proceeding to Day 30.

---

## Pre-flight: Verify Day 28

```bash
cargo test --workspace
grep -r "LlmCliHandler" crates/avix-core/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Benchmark Targets Summary

| Benchmark | Target |
|---|---|
| Boot to ready | < 700 ms |
| ATPToken validation | < 50 µs |
| IPC frame encode + decode | < 10 µs |
| IPC round-trip (local socket) | < 500 µs |
| VFS file read (in-memory) | < 50 µs |
| Tool registry lookup | < 5 µs |
| Provider adapter tool translation | < 5 µs |
| Tool name mangle | < 1 µs |
| Process table get | < 5 µs |

---

## Step 2 — Write / Complete Benchmarks

Ensure all benchmarks exist in `crates/avix-core/benches/`:

**`benches/all.rs`**

```rust
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use avix_core::{
    types::{Pid, tool::ToolName},
    ipc::frame,
    memfs::{MemFs, VfsPath},
    process::{ProcessEntry, ProcessKind, ProcessStatus, ProcessTable},
    tool_registry::{ToolRegistry, ToolEntry, ToolState, ToolVisibility},
    llm_svc::adapter::AnthropicAdapter,
    auth::atp_token::ATPToken,
};
use serde_json::json;

// ── ATPToken validation ───────────────────────────────────────────────────────

fn bench_atp_token_validate(c: &mut Criterion) {
    let claims = avix_core::auth::atp_token::ATPTokenClaims {
        session_id: "s".into(), identity_name: "alice".into(),
        role: avix_core::types::Role::Admin,
        expires_at: chrono::Utc::now() + chrono::Duration::hours(8),
    };
    let secret = "bench-secret-exactly-32-bytes-ok!!";
    let token = ATPToken::issue(claims, secret).unwrap();

    c.bench_function("atp_token_validate", |b| {
        b.iter(|| ATPToken::validate(&token, secret).unwrap())
    });
    // Target: < 50 µs
}

// ── IPC frame encode/decode ───────────────────────────────────────────────────

fn bench_ipc_frame(c: &mut Criterion) {
    let payload = json!({"jsonrpc": "2.0", "id": "bench-1", "method": "fs/read",
                         "params": {"path": "/users/alice/workspace/data.yaml"}});

    c.bench_function("ipc_frame_encode", |b| {
        b.iter(|| frame::encode(&payload).unwrap())
    });

    let encoded = frame::encode(&payload).unwrap();
    c.bench_function("ipc_frame_decode", |b| {
        b.iter(|| frame::decode::<serde_json::Value>(&encoded).unwrap())
    });
    // Target encode+decode combined: < 10 µs
}

// ── VFS read ─────────────────────────────────────────────────────────────────

fn bench_vfs_read(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = MemFs::new();
    let path = VfsPath::parse("/proc/57/status.yaml").unwrap();
    rt.block_on(async {
        fs.write(&path, b"status: running".to_vec()).await.unwrap();
    });

    c.bench_function("vfs_read", |b| {
        b.iter(|| rt.block_on(async { fs.read(&path).await.unwrap() }))
    });
    // Target: < 50 µs
}

// ── Tool registry lookup ──────────────────────────────────────────────────────

fn bench_tool_registry_lookup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let reg = ToolRegistry::new();
    rt.block_on(async {
        for i in 0..100u32 {
            reg.add("svc", vec![avix_core::tool_registry::ToolEntry {
                name: avix_core::types::tool::ToolName::parse(&format!("svc/tool-{i}")).unwrap(),
                owner: "svc".into(),
                state: ToolState::Available,
                visibility: ToolVisibility::All,
                descriptor: json!({}),
            }]).await.unwrap();
        }
    });

    c.bench_function("tool_registry_lookup", |b| {
        b.iter(|| rt.block_on(async { reg.lookup("svc/tool-42").await.unwrap() }))
    });
    // Target: < 5 µs
}

// ── Provider adapter translation ──────────────────────────────────────────────

fn bench_adapter_translate(c: &mut Criterion) {
    let adapter = AnthropicAdapter::new();
    let descriptor = json!({
        "name": "fs/write",
        "description": "Write to VFS",
        "input": {
            "path":    {"type": "string",  "required": true},
            "content": {"type": "string",  "required": true}
        }
    });

    c.bench_function("anthropic_translate_tool", |b| {
        b.iter(|| adapter.translate_tool(&descriptor))
    });
    // Target: < 5 µs
}

// ── Tool name mangle ──────────────────────────────────────────────────────────

fn bench_tool_name_mangle(c: &mut Criterion) {
    let name = ToolName::parse("mcp/github/list-prs").unwrap();

    c.bench_function("tool_name_mangle", |b| {
        b.iter(|| name.mangled())
    });
    // Target: < 1 µs
}

// ── Process table get ─────────────────────────────────────────────────────────

fn bench_process_table_get(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let table = ProcessTable::new();
    rt.block_on(async {
        for i in 0..1000u32 {
            table.insert(ProcessEntry {
                pid: Pid::new(i), name: format!("agent-{i}"), kind: ProcessKind::Agent,
                status: ProcessStatus::Running, parent: None, spawned_by_user: "alice".into(),
            }).await;
        }
    });

    c.bench_function("process_table_get", |b| {
        b.iter(|| rt.block_on(async { table.get(Pid::new(42)).await.unwrap() }))
    });
    // Target: < 5 µs
}

criterion_group!(
    benches,
    bench_atp_token_validate,
    bench_ipc_frame,
    bench_vfs_read,
    bench_tool_registry_lookup,
    bench_adapter_translate,
    bench_tool_name_mangle,
    bench_process_table_get,
);
criterion_main!(benches);
```

Update `crates/avix-core/Cargo.toml`:

```toml
[[bench]]
name    = "all"
harness = false

[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }
```

---

## Step 3 — Run and Verify All Benchmarks

```bash
cargo bench 2>&1 | tee bench-results.txt

# Check each target
grep "atp_token_validate"        bench-results.txt | grep -E "time:.*[0-9]+ µs"
grep "ipc_frame_encode"          bench-results.txt
grep "ipc_frame_decode"          bench-results.txt
grep "vfs_read"                  bench-results.txt
grep "tool_registry_lookup"      bench-results.txt
grep "anthropic_translate_tool"  bench-results.txt
grep "tool_name_mangle"          bench-results.txt
grep "process_table_get"         bench-results.txt
```

---

## Step 4 — Fix Any Misses

If any benchmark exceeds its target:

- **ATPToken >50µs**: Profile HMAC path — consider caching the signing key setup
- **VFS read >50µs**: Check lock contention — upgrade to `tokio::sync::RwLock` if not already in use
- **Tool registry >5µs**: Ensure lookup is read-only (`read().await`) with no write contention
- **Mangle >1µs**: Ensure `replace('/', "__")` is the only work — no allocation if possible

```bash
# After any fixes, run full test suite to confirm nothing broke
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

---

## Commit

```bash
# Include bench results in commit
git add -A
git add bench-results.txt
git commit -m "day-29: all benchmarks passing — atp, ipc, vfs, tool-registry, adapter, mangle"
```

## Success Criteria

- [ ] `atp_token_validate` < 50 µs
- [ ] `ipc_frame_encode` + `ipc_frame_decode` combined < 10 µs
- [ ] `vfs_read` < 50 µs
- [ ] `tool_registry_lookup` < 5 µs
- [ ] `anthropic_translate_tool` < 5 µs
- [ ] `tool_name_mangle` < 1 µs
- [ ] `process_table_get` < 5 µs
- [ ] All unit tests still pass after any optimisation changes

---
---

