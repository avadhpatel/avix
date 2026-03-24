---
name: implement-gap
description: Implement a development plan gap in the Avix codebase using TDD workflow. Use when assigned to close a specific dev plan gap.
license: MIT
---

# Skill: Implement a Dev Plan Gap

You are a coding agent working on the **Avix** codebase — a Rust workspace implementing
an agent operating system. Your job is to implement one dev plan gap from start to
finish: read the plan, write failing tests first, implement the code, verify everything
passes, then write a report.

---

## Mandatory reading before touching any code

Read these files in full before writing a single line:

1. `/home/avadh/workspace/avix/CLAUDE.md` — architecture invariants, conventions, and
   common mistakes. Violations are bugs, not style choices.
2. The dev plan file you are assigned (path given in your task prompt).
3. Any architecture docs referenced by the plan (`docs/architecture/`).
4. The existing code in the module(s) you will be modifying — never modify code you
   haven't read.

---

## Workflow — TDD, always

```
1. Read the dev plan fully.
2. Identify every type, function, and test the plan specifies.
3. Write the failing tests first (cargo test → watch them fail).
4. Write the minimum implementation to make each test pass.
5. Refactor if needed.
6. Run the full verification suite (see below).
7. Write a report.
```

**Never write implementation before a failing test exists.** No exceptions.

---

## Verification suite — all three must exit 0 before you report done

```bash
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace -- -D warnings
~/.cargo/bin/cargo fmt --check
```

If any of these fail, fix the issue. Do not report success while any of them are red.
Do not use `--no-verify`, `allow(warnings)`, or any other bypass.

---

## How to run individual tests during development

```bash
# Run only the crate you are working on
~/.cargo/bin/cargo test -p avix-client-core

# Run a specific test by name
~/.cargo/bin/cargo test -p avix-client-core notification::tests::add_increases_unread_count

# Run with output visible (useful when a test panics)
~/.cargo/bin/cargo test -p avix-client-core -- --nocapture
```

---

## Code conventions (from CLAUDE.md — enforce these)

| Rule | Detail |
|------|--------|
| **No `.unwrap()`** in non-test code | Use `?` or explicit error handling |
| **`thiserror`** for library errors | Already in `avix-client-core` as `ClientError` |
| **`anyhow`** for application errors | Use `ClientError::Other(anyhow!(...))` to wrap |
| **`tracing`** not `println!` | `info!`, `debug!`, `warn!`, `error!` in library code |
| **`tokio::test`** for async tests | Never use `std::thread::spawn` in async tests |
| **`tempfile::tempdir()`** for FS tests | Never write to real paths in tests |
| **`tokio::time::timeout`** | Wrap any async test that could hang |
| Struct/enum names | `PascalCase` |
| Function/variable names | `snake_case` |
| Constants | `SCREAMING_SNAKE_CASE` |

---

## Crate-specific guidance

### `avix-client-core`

- All public types must implement `Debug` (add manual impls for types holding
  `JoinHandle`, `broadcast::Receiver`, etc. — these don't auto-derive).
- All public store types (`NotificationStore`, etc.) must implement `Default` if they
  implement `new()`.
- `#[serde(tag = "type")]` internally-tagged enums: inner structs must use
  `#[serde(rename = "type", skip_deserializing, default)]` on any field named `type`
  — otherwise serde fails with "missing field" on deserialise.
- `save_json<T>` takes `T: Serialize + ?Sized` to accept `&[T]` slices.
- Async tests that use `tokio::spawn` internally (e.g. starting an `EventEmitter`) must
  be `#[tokio::test] async fn`, not `#[test] fn`.

### `avix-cli`

- This is a thin binary — no business logic. All protocol work goes in
  `avix-client-core`.
- To add `avix-client-core` as a dependency, add to `crates/avix-cli/Cargo.toml`:
  ```toml
  avix-client-core = { path = "../avix-client-core" }
  ```
- Use `clap` (already a workspace dep) for all CLI arg parsing.
- The `emit(json_mode, human_fn, value)` pattern separates human and JSON output —
  implement this helper once and use it everywhere.

---

## What to do when a test references a `todo!()`

Stub tests with `todo!()` bodies that require infrastructure not yet built (e.g. a mock
WS transport) should be marked `#[ignore = "reason"]` rather than left to panic. Write
the reason clearly so a future agent knows exactly what is needed to implement them.

---

## Reporting

When done, write a report to:

```
/home/avadh/workspace/avix/.agents/reports/<gap-name>-<YYYY-MM-DD>.md
```

Follow the report format defined in `.agents/skills/report-format/SKILL.md`.

---

## Common mistakes to avoid in this codebase

| Mistake | Fix |
|---------|-----|
| Calling `dispatcher.call(cmd)` with owned `Cmd` | Signature is `call(&self, cmd: &Cmd)` |
| `connect_async` error mapped as `(e, _)` tuple | It returns a plain `Error`, not a tuple |
| `let mut x: WsSink = mutex.lock().await` | Remove explicit type — `MutexGuard` deref handles it |
| Escaped quotes `\\\"` in string literals | These are invalid Rust; use `\"` directly |
| `chrono` in `[dev-dependencies]` when used in lib code | Move to `[dependencies]` |
| `#[tokio::test] fn` (non-async) | Either `#[test] fn` or `#[tokio::test] async fn` |
| Closure borrows outer `Arc` without `move` | Clone the `Arc` first, then `move` into the closure |
| `Frame` inner structs with `#[serde(rename = "type")]` | Add `skip_deserializing, default` when enum uses `#[serde(tag = "type")]` |