# Avix Agent Workspace

This folder contains skills and reports for coding agents working on the Avix codebase.

---

## For an agent picking up a task

**Step 1 — Read these files before touching any code (in order):**

1. `/home/avadh/workspace/avix/CLAUDE.md` — architecture invariants, conventions,
   crate structure. This overrides everything else.
2. `.agents/skills/implement-gap/SKILL.md` — the full workflow for implementing a dev plan.
3. `.agents/skills/rust-best-practices/SKILL.md` — Rust patterns enforced in this codebase.
4. `.agents/skills/testing-in-avix/SKILL.md` — how to write and run tests here.
5. The dev plan you are assigned (`docs/dev_plans/<gap-name>.md`).

**Step 2 — Follow the TDD workflow in `implement-gap.md` exactly.**

**Step 3 — Verify with all three checks before reporting done:**
```bash
~/.cargo/bin/cargo test --workspace
~/.cargo/bin/cargo clippy --workspace -- -D warnings
~/.cargo/bin/cargo fmt --check
```

**Step 4 — Write a report** following `.agents/skills/report-format.md` to
`.agents/reports/<gap-name>-<YYYY-MM-DD>.md`.

---

## Skills

| File | Purpose |
|------|---------|
| `skills/implement-gap/SKILL.md` | Full workflow: read plan → TDD → verify → report |
| `skills/rust-best-practices/SKILL.md` | Rust patterns, error handling, async, serde |
| `skills/testing-in-avix/SKILL.md` | How to write, run, and structure tests |
| `skills/report-format/SKILL.md` | Required report format and file naming |

---

## Reports

Completed reports live in `reports/`. Each report names the gap, date, status, files
changed, test results, and any bugs found in passing. Read recent reports before
starting a new gap — they contain bugs and gotchas found by previous agents.

---

## Active dev plans (next up)

| Gap | File | Depends on |
|-----|------|------------|
| D tests | `docs/dev_plans/client-gap-D-notification-store-hil-persistence.md` | — |
| F | `docs/dev_plans/client-gap-F-cli-atp-connect-scripting.md` | E (done) |
| G | `docs/dev_plans/client-gap-G-cli-tui-skeleton.md` | F |
| H | `docs/dev_plans/client-gap-H-cli-tui-live-events-hil.md` | C, D, G |
