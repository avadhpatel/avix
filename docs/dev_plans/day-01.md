# Day 1 — Repository Setup & Documentation Structure

> **Goal:** Create the Git repository with the complete workspace skeleton, documentation scaffolding, CI/CD pipeline, and `CLAUDE.md`. No implementation code yet — this day is purely about creating the project foundation that every subsequent day builds on.

---

## Pre-flight Checklist (Day 1 has no previous day to verify)

This is Day 1. Confirm the following environment prerequisites before proceeding:

```bash
# Rust toolchain >= 1.78 (required for async trait stabilisation)
rustc --version
cargo --version

# Git
git --version

# cargo-tarpaulin (coverage)
cargo install cargo-tarpaulin --locked

# cargo-watch (watch mode)
cargo install cargo-watch --locked

# Confirm you are in the directory where the repo will live
pwd
```

All commands must succeed without error before continuing.

---

## Step 1 — Initialise the Git Repository

```bash
mkdir avix && cd avix
git init
git checkout -b main
```

---

## Step 2 — Create the Cargo Workspace

Create `Cargo.toml` at the repo root:

```toml
# Cargo.toml — workspace root
[workspace]
members = [
    "crates/avix-core",
    "crates/avix-cli",
    "crates/avix-app",
    "crates/avix-docker",
]
resolver = "2"

[workspace.package]
version     = "0.1.0"
edition     = "2021"
authors     = ["Avix Contributors"]
license     = "MIT"
repository  = "https://github.com/avix-os/avix"

[workspace.dependencies]
tokio       = { version = "1",  features = ["full"] }
serde       = { version = "1",  features = ["derive"] }
serde_json  = { version = "1" }
serde_yaml  = { version = "0.9" }
tracing     = { version = "0.1" }
tracing-subscriber = { version = "0.3", features = ["json"] }
anyhow      = { version = "1" }
thiserror   = { version = "1" }
uuid        = { version = "1",  features = ["v4"] }
hmac        = { version = "0.12" }
sha2        = { version = "0.10" }
hex         = { version = "0.4" }
chrono      = { version = "0.4", features = ["serde"] }
tempfile    = { version = "3" }
```

---

## Step 3 — Scaffold the Four Crates

```bash
cargo new --lib crates/avix-core
cargo new      crates/avix-cli
cargo new      crates/avix-app
cargo new      crates/avix-docker
```

Add minimal `Cargo.toml` to each crate:

**`crates/avix-core/Cargo.toml`**

```toml
[package]
name    = "avix-core"
version.workspace = true
edition.workspace = true

[dependencies]
tokio.workspace      = true
serde.workspace      = true
serde_json.workspace = true
serde_yaml.workspace = true
tracing.workspace    = true
anyhow.workspace     = true
thiserror.workspace  = true
uuid.workspace       = true
hmac.workspace       = true
sha2.workspace       = true
hex.workspace        = true
chrono.workspace     = true

[dev-dependencies]
tempfile.workspace = true
tokio    = { workspace = true, features = ["test-util"] }
```

**`crates/avix-cli/Cargo.toml`**

```toml
[package]
name    = "avix-cli"
version.workspace = true
edition.workspace = true

[[bin]]
name = "avix"
path = "src/main.rs"

[dependencies]
avix-core   = { path = "../avix-core" }
tokio.workspace      = true
serde_json.workspace = true
anyhow.workspace     = true
```

**`crates/avix-app/Cargo.toml`** and **`crates/avix-docker/Cargo.toml`** — identical pattern to `avix-cli`, different binary names (`avix-app`, `avix-headless`).

---

## Step 4 — Create the Documentation Tree

```bash
mkdir -p docs/architecture
mkdir -p docs/development
mkdir -p docs/user

# Architecture docs (stubs — will be filled in as work progresses)
touch docs/architecture/00-overview.md
touch docs/architecture/01-filesystem.md
touch docs/architecture/02-bootstrap.md
touch docs/architecture/03-ipc.md
touch docs/architecture/04-atp.md
touch docs/architecture/05-capabilities.md
touch docs/architecture/06-agents.md
touch docs/architecture/07-services.md
touch docs/architecture/08-llm-service.md
touch docs/architecture/09-runtime-executor-tools.md

# Development docs
touch docs/development/setup.md
touch docs/development/testing.md
touch docs/development/tdd-workflow.md
touch docs/development/benchmarking.md

# User docs
touch docs/user/quickstart.md
touch docs/user/installation.md
touch docs/user/tutorial.md
```

---

## Step 5 — Write `CLAUDE.md`

Create `CLAUDE.md` at the repo root. This file instructs Claude Code on every session:

```markdown
# CLAUDE.md — Avix Development Instructions

## What is Avix?
Avix is an agent operating system modelled on Unix/Linux primitives. Agents run as
processes (PIDs), the LLM is the CPU, and familiar OS abstractions (filesystem,
signals, IPC, capabilities) are mapped onto agentic concepts.

**Authoritative references:**
- Architecture: `docs/architecture/` (all files)
- This file: development conventions and invariants

## Architecture Invariants (never violate)

1. `auth.conf` must exist before Avix starts. No setup-gate inside core.
2. `credential.type: none` does not exist. All auth is `api_key` or `password`.
3. ATP = external (WebSocket). IPC = internal (local sockets + JSON-RPC 2.0).
4. IPC transport is `local-ipc` — Unix sockets on Linux/macOS, Named Pipes on Windows.
5. Services are language-agnostic. They speak JSON-RPC 2.0 over local sockets.
6. Router opens a fresh connection per tool call — one connection, one call.
7. Long-running tools return `job_id` immediately; workers emit via jobs.svc.
8. `llm.svc` owns all AI inference. RuntimeExecutor never calls provider APIs directly.
9. Tool names use `/` as namespace separator. Provider adapters mangle to `__` on the
   wire and unmangle on return. No Avix tool name contains `__`.
10. Category 2 tools (agent/, pipe/, cap/, job/) are registered by RuntimeExecutor at
    agent spawn via `ipc.tool-add` and removed at exit via `ipc.tool-remove`.
11. Secrets in `/secrets/` are never readable via the VFS — kernel-injected only.
12. Kernel tool calls are deterministic — never LLM-decided.

## Development Conventions

### TDD — Tests First, Always
Write the test. Watch it fail. Implement until it passes. Never write implementation
code without a failing test already existing.

### Code Organisation
- All logic lives in `avix-core` as a library.
- `avix-cli`, `avix-app`, `avix-docker` are thin binary wrappers.
- No business logic in binary crates.

### Naming Conventions
- Structs/enums: `PascalCase`
- Functions/variables: `snake_case`
- Constants: `SCREAMING_SNAKE_CASE`
- IPC method names: `namespace/verb` (e.g. `fs/read`, `kernel/proc/spawn`)
- Config `kind` values: `PascalCase` (e.g. `KernelConfig`, `AuthConfig`)

### Error Handling
- Use `thiserror` for library error types in `avix-core`.
- Use `anyhow` for application-level errors in binary crates.
- Never use `.unwrap()` in non-test code. Use `?` or explicit error handling.
- Every public function that can fail returns `Result<T, E>`.

### Async
- Use `tokio::test` for async tests.
- Use `Arc<RwLock<T>>` (tokio) for shared mutable state.
- Prefer `tokio::spawn` for background tasks; hold the `JoinHandle`.

### Logging
- Use the `tracing` crate everywhere.
- Structured JSON output in production; pretty output in dev.
- Never use `println!` in library code. Use `tracing::info!`, `tracing::debug!`, etc.

### Testing
- Target: 95%+ coverage via `cargo tarpaulin`.
- Unit tests in the same file under `#[cfg(test)]`.
- Integration tests in `crates/avix-core/tests/`.
- Use `tempfile::tempdir()` for any test that needs a filesystem root.

### Commit Convention
Each day's work is a single commit: `day-NN: <short description>`.

## Performance Targets
| Operation                      | Target    |
|--------------------------------|-----------|
| Boot to ready                  | < 700 ms  |
| ATPToken validation            | < 50 µs   |
| IPC frame encode + decode      | < 10 µs   |
| IPC round-trip (local)         | < 500 µs  |
| VFS file read (in-memory)      | < 50 µs   |
| Tool registry lookup           | < 5 µs    |
| Provider adapter tool translation | < 5 µs |
| Tool name mangle               | < 1 µs    |
```

---

## Step 6 — Write Root `README.md`

```markdown
# Avix — Agent Operating System

Avix is an agent OS modelled on Unix primitives. Agents run as processes,
the LLM is the CPU, and familiar abstractions (filesystem, signals, IPC,
capabilities) are applied to agentic concepts.

## Quick Start

```bash
# Install
cargo build --release

# Initialise config
./target/release/avix config init

# Start
./target/release/avix start
```

## Documentation

See `docs/architecture/` for the full architecture reference.

## Development

See `CLAUDE.md` for development conventions.
Run `cargo test` to run all tests.
Run `cargo tarpaulin --out Html` for coverage.
```

---

## Step 7 — Set Up CI/CD

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  test:
    name: Test
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace

  clippy:
    name: Clippy
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace -- -D warnings

  fmt:
    name: Format
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --check

  coverage:
    name: Coverage
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo install cargo-tarpaulin --locked
      - run: cargo tarpaulin --workspace --out Xml
      - uses: codecov/codecov-action@v4
```

Create `.github/workflows/coverage.yml` — same as above but triggered on schedule for nightly coverage reports.

---

## Step 8 — Add `.gitignore`

```
/target
Cargo.lock
*.enc
.env
*.log
tarpaulin-report.html
```

---

## Step 9 — Write the Placeholder Test

Every crate must compile and have at least one passing test before committing.

In `crates/avix-core/src/lib.rs`:

```rust
//! avix-core — all Avix logic lives here as a library.

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_compiles() {
        // Day 1 placeholder — removed when real types are added on Day 2.
        assert!(true);
    }
}
```

---

## Step 10 — Verify

```bash
# Everything must pass before committing

cargo build --workspace
# Expected: Compiling avix-core, avix-cli, avix-app, avix-docker — 0 errors

cargo test --workspace
# Expected: test workspace_compiles ... ok

cargo clippy --workspace -- -D warnings
# Expected: 0 warnings

cargo fmt --check
# Expected: no diff output (exit 0)
```

---

## Commit

```bash
git add -A
git commit -m "day-01: repo setup, workspace skeleton, CLAUDE.md, CI"
```

---

## Success Criteria

- [ ] `cargo build --workspace` exits 0
- [ ] `cargo test --workspace` — all tests pass
- [ ] `cargo clippy --workspace -- -D warnings` exits 0
- [ ] `cargo fmt --check` exits 0
- [ ] `docs/architecture/` contains 10 stub files
- [ ] `CLAUDE.md` exists at repo root
- [ ] `.github/workflows/ci.yml` exists
- [ ] Single commit tagged `day-01`
