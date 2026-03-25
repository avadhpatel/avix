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

### Client (avix-client-core + avix-cli)

| Gap | File | Depends on |
|-----|------|------------|
| D tests | `docs/dev_plans/client-gap-D-notification-store-hil-persistence.md` | — |
| F | `docs/dev_plans/client-gap-F-cli-atp-connect-scripting.md` | E (done) |
| G | `docs/dev_plans/client-gap-G-cli-tui-skeleton.md` | F |
| H | `docs/dev_plans/client-gap-H-cli-tui-live-events-hil.md` | C, D, G |

### Service authoring (avix-core)

| Gap | File | Depends on |
|-----|------|------------|
| svc-A | `docs/dev_plans/svc-gap-A-service-unit-parser.md` | — |
| svc-B | `docs/dev_plans/svc-gap-B-service-process-spawner.md` | svc-A |
| svc-C | `docs/dev_plans/svc-gap-C-tool-descriptor-scanner.md` | svc-A, svc-B |
| svc-D | `docs/dev_plans/svc-gap-D-service-installer.md` | svc-A |
| svc-E | `docs/dev_plans/svc-gap-E-cli-service-commands.md` | svc-D, client-F |
| svc-F | `docs/dev_plans/svc-gap-F-ipc-tool-add-remove-wire.md` | svc-A, svc-B, svc-C |
| svc-G | `docs/dev_plans/svc-gap-G-caller-injection.md` | svc-A, svc-F |
| svc-H | `docs/dev_plans/svc-gap-H-restart-watchdog-secrets.md` | svc-A, svc-B |
