# Day 31 — Documentation, Coverage Gate & Release

> **Goal:** Write/complete the two new architecture docs (`08-llm-service.md` and `09-runtime-executor-tools.md`), verify 95%+ test coverage, pass all CI checks, complete the release checklist, and tag `v0.1.0`.

---

## Pre-flight: Verify Day 30

```bash
cargo test --workspace
cargo build -p avix-app
grep -r "AgentSpawn" crates/avix-app/src/
cargo clippy --workspace -- -D warnings
```

---

## Step 1 — Generate Coverage Report

```bash
cargo tarpaulin --workspace --out Html --output-dir coverage/

# Check the aggregate
cargo tarpaulin --workspace --out Stdout 2>/dev/null | grep "Coverage"
# MUST read >= 95% for avix-core

# Open coverage/tarpaulin-report.html and inspect any uncovered lines
```

If coverage is below 95%, write tests for uncovered paths before proceeding. Common gaps:

- Error path branches in config parsers
- Signal delivery to unsubscribed PIDs
- Edge cases in tool name parsing
- Bootstrap phase error paths

---

## Step 2 — Write `docs/architecture/08-llm-service.md`

Derived from the `LLM_service_spec` project file. Sections:

```markdown
# 08 — llm.svc: Multi-Modality LLM Service

## Overview
## Configuration (/etc/avix/llm.yaml)
## Providers (Anthropic, OpenAI, Ollama, Stability AI, ElevenLabs)
## Authentication (api_key, oauth2, none)
## Routing Engine (defaultProviders, explicit override, modality validation)
## Tool Name Mangling (/ → __)
## Binary Output Handling (scratch dir, VFS path return)
## OAuth2 Refresh (background loop, SIGHUP reload)
## Health Checks (tool.changed events)
## Capability Scopes (llm:inference, llm:image, llm:speech, ...)
## Error Codes (-32010 through -32020)
## IPC Interface (tool descriptors, tool calls, tool results)
```

---

## Step 3 — Write `docs/architecture/09-runtime-executor-tools.md`

Derived from the `Runtime_executor_tool_exposure_spec` project file. Sections:

```markdown
# 09 — RuntimeExecutor Tool Exposure Model

## Overview
## Three Tool Categories
  ### Category 1: Direct Tools
  ### Category 2: Avix Behaviour Tools
  ### Category 3: Transparent Tools
## Capability-to-Tool Mapping
## Always-Present Tools (cap/request-tool, cap/escalate, cap/list, job/watch)
## Category 2 Registration Lifecycle (ipc.tool-add at spawn, ipc.tool-remove at exit)
## Tool Visibility (ToolVisibility::User scoping)
## System Prompt Block Construction (Blocks 1–4)
## The 7-Step Turn Loop
## Tool Name Mangling in Practice
```

---

## Step 4 — Final Release Checklist

```bash
# 1. All tests pass
cargo test --workspace
echo "Exit: $?"  # Must be 0

# 2. No clippy warnings
cargo clippy --workspace -- -D warnings
echo "Exit: $?"  # Must be 0

# 3. Formatting is clean
cargo fmt --check
echo "Exit: $?"  # Must be 0

# 4. Coverage >= 95%
cargo tarpaulin --workspace --out Stdout 2>/dev/null | grep "Coverage"
# Read and confirm >= 95.0%

# 5. All benchmarks pass targets
cargo bench 2>&1 | grep -E "time:.*[0-9\.]+ [nµm]s" | head -20

# 6. avix config init works end-to-end
cargo build --release 2>/dev/null
TMPDIR=$(mktemp -d)
./target/release/avix config init --root "$TMPDIR"
ls "$TMPDIR/etc/auth.conf"  # Must exist

# 7. avix llm status works
# (requires a running avix instance — manual check or integration test)

# 8. Documentation complete
ls docs/architecture/08-llm-service.md
ls docs/architecture/09-runtime-executor-tools.md
wc -l docs/architecture/08-llm-service.md   # Should be > 50 lines
wc -l docs/architecture/09-runtime-executor-tools.md  # Should be > 50 lines
```

---

## Step 5 — Functional Demo Tests

```bash
# Multi-modality pipeline demo (text → image → speech)
# This test exercises the llm.svc routing across three modalities
cargo test -p avix-core --test llm_svc -- multimodality_pipeline 2>/dev/null || echo "Add test if missing"

# Category 2 tool registration lifecycle
cargo test -p avix-core --test runtime_executor -- shutdown_deregisters_all_category2_tools

# avix llm status shows all providers
cargo test -p avix-core --test llm_cli -- llm_status_returns_all_providers
```

---

## Step 6 — Tag the Release

```bash
git add -A
git commit -m "day-31: docs 08 and 09, 95%+ coverage verified, release checklist complete"

git tag -a v0.1.0 -m "Avix v0.1.0 — prototype milestone

- 32 kernel syscalls across 6 domains
- Full RuntimeExecutor turn loop with HIL (3 scenarios)
- Multi-modality llm.svc (5 provider adapters)
- Category 2 tool registration lifecycle
- MemFS VFS with <50µs reads
- ATP gateway over WebSocket
- AES-256-GCM secrets store
- Cron scheduler
- Snapshot/restore
- exec.svc + mcp-bridge.svc
- 95%+ test coverage"

git push origin main --tags
```

---

## Success Criteria

- [ ] `cargo test --workspace` exits 0
- [ ] `cargo clippy --workspace -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `cargo tarpaulin` reports ≥ 95% for `avix-core`
- [ ] All Day 29 benchmark targets confirmed still passing
- [ ] `docs/architecture/08-llm-service.md` > 50 lines
- [ ] `docs/architecture/09-runtime-executor-tools.md` > 50 lines
- [ ] `avix config init` creates a valid `auth.conf`
- [ ] `avix llm status` lists all configured providers
- [ ] Category 2 tool deregistration lifecycle test passes
- [ ] `git tag v0.1.0` created and pushed
- [ ] CI pipeline (GitHub Actions) goes green on the tag
