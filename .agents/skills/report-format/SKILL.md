---
name: report-format
description: Follow the exact report format for Avix agent task completion. Use when finishing an implementation task.
license: MIT
---

# Skill: Report Format

Every coding agent must write a report after completing an implementation task.
This is the required format.

---

## File location

```
/home/avadh/workspace/avix/.agents/reports/<gap-name>-<YYYY-MM-DD>.md
```

Examples:
- `client-gap-D-notification-tests-2026-03-24.md`
- `client-gap-F-cli-atp-subcommands-2026-03-25.md`

---

## Required sections

```markdown
# <Gap Name> — Implementation Report
Date: YYYY-MM-DD
Status: COMPLETE | PARTIAL | BLOCKED

## What was implemented

<Bullet list of every file created or modified, with a one-line description of
what changed. Include the full relative path.>

- `crates/avix-client-core/src/notification.rs` — added 8 tests for NotificationStore
- `crates/avix-client-core/src/persistence.rs` — added 3 tests for save/load/atomic write

## Test results

<Paste the final `cargo test --workspace` summary lines only — one line per test binary.>

test result: ok. 21 passed; 0 failed; 6 ignored; ...
test result: ok. 740 passed; 0 failed; ...

## Clippy

PASS  (zero warnings with -D warnings)
  — or —
FAIL  <paste the error lines>

## Remaining gaps / ignored tests

<List any tests left as #[ignore] and why. List any parts of the dev plan
that were not implemented and why.>

- `dispatcher::tests::call_returns_matching_reply` — ignored: requires mock WS transport
- `event_emitter::tests::subscribe_all_receives_forwarded_events` — ignored: same

## Bugs found during implementation

<List any bugs in existing code that you found and fixed, even if they were
outside the scope of your gap. Include the file, line, and a one-line description.>

- `notification.rs:63` — escaped quotes `\\\"` caused compile error; fixed to `\"`
- `Cargo.toml` — `chrono` was in `[dev-dependencies]`; moved to `[dependencies]`

## Notes for the next agent

<Anything a future agent needs to know: known issues, non-obvious design decisions,
or prerequisites for the next gap.>
```

---

## What makes a good report

- **Precise file paths** — relative to the workspace root.
- **Actual test output** — copy-pasted, not paraphrased.
- **Honest about ignored tests** — list every `#[ignore]` with the reason.
- **Bugs found** — even if you fixed them in passing. This is the audit trail.
- **Short notes section** — one paragraph maximum. The code is the ground truth;
  the notes are only for non-obvious decisions.

---

## What to do if blocked

If you cannot complete the gap (missing dependency, ambiguous spec, upstream bug):

1. Set `Status: BLOCKED` in the report.
2. Describe exactly what is blocking you in the **Notes** section.
3. Do not leave the codebase in a broken state — revert any partial changes that
   prevent `cargo test --workspace` from passing.
4. Commit the report alone with message: `docs: agent report — <gap-name> BLOCKED`.