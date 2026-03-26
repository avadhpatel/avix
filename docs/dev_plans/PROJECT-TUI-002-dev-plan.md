# PROJECT-TUI-002-dev-plan.md: TUI Polish Gaps Implementation

## Overview
4 small, independent TDD gaps from TUI-001/PM-REPORT-PROJECT-TUI-002-INIT. Sequential: G1 dispatch → G2/G3 status → G4 clean. ~4-6h total. No layout changes.

## What to Implement (Sequential Tasks)
* **G1 P2: :kill Dispatch** (app.rs): Impl dispatch_parsed_command Kill → send_signal SIGKILL + log + error notif.
* **G2 P3: Status Hint** (status.rs): Append hint if !command_mode.
* **G3 P4: Uptime** (state.rs/app.rs/status.rs): startup_time + fmt mm:ss.
* **G4 Low: Clippy** (tui/): cargo clippy fixes.

## TDD Approach
Per gap:
* Failing test → minimal impl → tracing → verify.
* `cargo test --lib avix-cli::tui`.

G1: `#[test] fn dispatch_kill_calls_send_signal() { mock_dispatcher; assert_send_signal_called(pid, \"SIGKILL\"); }`
G2: Render test: assert text contains hint when !cmd_mode.
G3: `#[test] fn uptime_formats_correctly() { ... }`
G4: `cargo clippy` pre/post.

## Detailed Implementation Guidance
* **Crates/Files**:
  | Gap | Files | Key Changes |
  |-----|-------|-------------|
  | G1 | app.rs | +use commands::send_signal; match Kill{..} → log SentCommand → if dispatcher { send_signal(..).await? → notif } |
  | G2 | widgets/status.rs | render(): let hint=if !state.command_mode {\" | Press / ...\"} else {\"\"}; format!(.., hint) |
  | G3 | state.rs | +pub startup_time: Instant; app.rs: state.startup_time=Instant::now(); status.rs: let elapsed=..; format!(\"{:02}:{:02}\", mins,secs) |
  | G4 | tui/* | cargo clippy -p avix-cli --fix; manual #[allow] removes |

* **Tracing**: `debug_span!(\"tui.dispatch.kill\", pid=?pid);`
* **Errors**: ATP err → Notification::SysAlert.
* **CLAUDE.md**: No unwrap!, anyhow::Result, tokio::test.
* **Verify**: Post-G1 manual kill; uptime ticks in loop.

## Testing Requirements
* **Unit**: dispatch_kill, parse_kill (exists), render_hint/uptime (new), reducer noop.
* **Integration**: Mock SharedState/AtpClient → assert send_signal called.
* **Manual**:
  * `avix tui` → / :kill &lt;real_pid&gt; → agent gone + log.
  * Status: hint visible → / → gone; uptime increments.
  * Resize 80x24.
* **E2E**: No regressions (spawn/HIL/logs).

## Usability Considerations
* Usability-agent checkpoints: Hint reduces magic; kill intuitive; uptime glanceable.
* Priorities: G1 functional → G2 discoverability → G3/G4 polish.

## Estimated Effort & Priority
* Effort: Low (small fixes). G1:2h, G2/G3:1h ea, G4:0.5h.
* Priority: High (TUI-001 blocker).

## Feedback Integration
* From PM-REPORT-PROJECT-TUI-002-INIT + usability gaps in tui.md.
* Post: Incorporate coding/testing/usability reports.

## Completion Checklist
- [ ] G1-G4 impl'd, `cargo test` passes
- [ ] `cargo clippy -p avix-cli -D warnings` clean
- [ ] Coverage tui/ >95%
- [ ] No TUI regressions (manual 5min)
- [ ] Usability-agent: APPROVED
- [ ] Hand-off: \"TUI-002 complete, ready docs/program-manager\"