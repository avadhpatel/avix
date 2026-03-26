# PROJECT-TUI-001-dev-plan.md: Implementation Plan for TUI Improvements

## Overview
Step-by-step breakdown to implement the spec: extend state/actions, add parser/widgets, update layout/key handling/event hooks. TDD workflow: failing test → minimal impl → refactor. Target: zero regressions, full coverage. Estimated 8-12 coding gaps, 2-3 days total.

## What to Implement (Sequential Tasks)
1. **Extend TuiState & Action** (`state.rs`): Add CommandInputState, EventLog, flags, new Action variants, reducer logic.
2. **Command Parser** (new `crates/avix-cli/src/tui/parser.rs`): `parse(input: &str) -> Result<ParsedCommand, String>`.
3. **StatusWidget** (new `widgets/status.rs`): Multi-line stats display.
4. **CommandBarWidget** (new `widgets/command_bar.rs`): Input render + cursor/history.
5. **EventLogWidget** (new `widgets/event_log.rs`): Circular list render.
6. **HelpModalWidget** (new `widgets/help_modal.rs`): Command list modal.
7. **Update ui() layout** (`app.rs`): Responsive vertical/horizontal splits, conditional log/cmdbar.
8. **Cmd mode key handling** (`app.rs`): '/', input deltas, submit → parse → dispatch → log.
9. **Magic key aliases** (`app.rs`): Map 'q'→Quit etc. for compat.
10. **Event logging hooks** (`app.rs dispatch_event`, submit): Log Sent/Received.
11. **Toggle commands** (`app.rs`): :logs, :notifs, :new-agent-form.
12. **Full integration tests** + manual verification.

## TDD Approach
For each task:
* Write **failing test** (unit for state/parser/widget, expect render output or reducer effect).
* Impl minimal to pass.
* Refactor + add tracing::debug!.
* Success: `cargo test --lib avix-cli` passes.

Examples:
* Task 1: `#[test] fn reducer_enter_cmd_sets_mode() { ... assert!(state.command_mode); }`
* Task 2: `#[test] fn parse_spawn_quotes() { assert_eq!(parse(\":spawn foo \\\"g o a l\\\"\"), Ok(Spawn{goal:\"g o a l\"})); }`
* Task 7: Manual resize test (no snapshot, visual).

## Detailed Implementation Guidance
* **Crates/Files**:
  | Task | Files | Key Changes |
  |------|--------|-------------|
  | 1 | state.rs | #[derive(Clone)] structs, reducer match arms w/ EventLog truncate(10). |
  | 2 | tui/parser.rs | fn parse_line(s: &str) -> Result<ParsedCommand>; manual split/trim/quote (no nom/regex). |
  | 3-6 | widgets/*.rs | Match existing style (AgentListWidget etc.): render(Rect) -> Widget, select_next/prev. |
  | 7 | app.rs ui() | Layout::default().direction(Vertical).constraints([Len(3), Perc(20), Min(10), Len(if cmd_mode{2}else{0})]).split(size); |
  | 8 | app.rs key match | if state.command_mode { match key.code { Char(c) => Action::UpdateInput(Char(c)), ... } } |
  | 10 | dispatch_event() | let summary = format!(\"{:?} pid={:?} {}\", event.kind, body.pid, body.text); action_tx.send(LogEvent(..)).await; |

* **Tracing/Logging**: `debug_span!(\"tui.cmd\", input=%state.command_input.as_ref().map(|s|s.input))`.
* **Errors**: Parse/ATP → Notification::SysAlert via shared.notifications.add().
* **CLAUDE.md Alignment**: No unwrap/println!, tokio::sync, Arc<RwLock> unchanged, tests tokio::test.
* **Extensibility**: ParsedCommand enum open for future cmds (:pipe etc.).

## Testing Requirements
* **Unit** (state.rs, parser.rs, widgets/*.rs): 95% coverage. Reducer effects, parse roundtrips (20+ cases incl invalid/quotes/empty), render non-panic.
* **Integration** (app.rs): Mock SharedState/AtpClient, simulate key events → assert actions/dispatches.
* **Manual**:
  * Terminal 80x24/120x40 resize → no crash, adaptive.
  * Full flow: :connect → :spawn → output → HIL modal → :kill → :logs → :help.
  * Edge: invalid cmd, long input, history nav.
* **E2E**: Run `avix tui`, interact, check no regressions (HIL a/d/esc).

## Usability Considerations
* Post-impl: Delegate to usability-agent for real-user simulation (CLI op).
* Checkpoints: Cmd discoverable (`/` obvious?), input fluid (quotes/arrows), log useful (truncate smart), small-term ok.
* Priorities: 1. No friction in existing flows, 2. Cmd mode intuitive, 3. Logs glanceable.

## Estimated Effort & Priority
* **Effort**: High (new UX paradigm, layout rework). ~20h coding/tests + 4h usability/debug.
* **Priority**: High — unlocks discoverable TUI, blocks advanced CLI scripting.

## Feedback Integration
* Initial plan — no prior agent/human feedback.
* Post-coding: Incorporate coding-agent decision log, testing-agent bugs/observability, usability-agent UX gaps.

## Completion Checklist
- [ ] All tasks implemented, tests pass (`cargo test --workspace`)
- [ ] Clippy/fmt clean
- [ ] Coverage >90% tui/
- [ ] No TUI regressions (HIL, forms, notifs)
- [ ] Usability-agent report: APPROVED
- [ ] Hand-off to program-manager-agent: \"TUI improvements complete, ready for docs.\"