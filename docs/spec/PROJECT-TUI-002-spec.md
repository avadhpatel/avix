# PROJECT-TUI-002-spec.md: TUI Polish — Close Known Gaps from TUI-001

## Version
1.0 (2024-10-27) — Initial spec for post-TUI-001 gaps.

## Motivation & Problem Statement
PROJECT-TUI-001 delivered core TUI dashboard with command mode (`/` + `:` commands), event logging, responsive layout, modals, and ATP integration. Usability-agent and architecture/tui.md identified 4 small remaining gaps blocking production-readiness:

* G1 (P2): `:kill &lt;pid&gt;` parses but dispatches stub notification (TODO in app.rs).
* G2 (P3): Status bar lacks discoverability hint for command mode (`/cmd :help`).
* G3 (P4): Uptime placeholder `--:--:--` in status (TODO in status.rs).
* G4 (Low): Minor clippy lints in avix-cli::tui (unused vars, etc.).

These are polish items: no layout/API changes, pure TDD fixes. Aligns with CLAUDE.md invariants (tracing spans, no unwrap/println!, client-side only).

## Goals
* G1: Full `:kill &lt;pid&gt;` → `send_signal(dispatcher, credential, pid, \"SIGKILL\", None)` dispatch + SentCommand log + success/error notif.
* G2: StatusWidget appends \" | Press / for commands (:help)\" if !command_mode.
* G3: TuiState.startup_time: Instant; StatusWidget formats elapsed as \"mm:ss\" (update on every tick).
* G4: Zero clippy warnings (`cargo clippy -p avix-cli -D warnings` clean).
* Backward compat: No regressions in existing flows (spawn, HIL, logs, etc.).
* Observability: `tracing::debug_span!(\"tui.kill\", pid=%pid)` etc.

## Non-Goals
* New commands/modals/layouts.
* Persistent uptime (session-only).
* Clippy in other crates.
* Chrono dep (manual formatting).
* kill_agent wrapper (use existing send_signal).

## Architecture Impact
Client-side only (`crates/avix-cli/src/tui/*`). No ATP/IPC/HIL changes.

* state.rs: +`startup_time: Instant`.
* app.rs: dispatch_parsed_command Kill arm → send_signal; init startup_time.
* widgets/status.rs: elapsed fmt; conditional hint.
* parser.rs: Kill already parses u64 pid ✓.
* Tracing: Spans in dispatch paths.
* Perf: Negligible (fmt cheap, tick 100ms).

Per docs/architecture/00-overview.md: Fresh IPC per call, capability token scoped.

## Detailed Design

### G1: Implement :kill Dispatch (app.rs:dispatch_parsed_command)
```rust
ParsedCommand::Kill { pid } => {
    let cmd_str = format!("kill {}", pid);
    let log_event = TuiEvent::SentCommand { cmd: cmd_str, timestamp: Instant::now() };
    action_tx.send(Action::LogEvent(log_event)).await;
    if let Some(dispatcher) = &shared_state.read().await.dispatcher {
        match send_signal(dispatcher, &client_config.credential, pid, "SIGKILL", None).await {
            Ok(()) => { /* success notif? */ },
            Err(e) => Notification::from_sys_alert("error", &format!("kill {}: {}", pid, e)),
        }
    }
}
```
* Import `use avix_client_core::commands::send_signal;`.
* Errors → SysAlert notif + log.

### G2: Status Hint (widgets/status.rs:render)
```rust
let hint = if !state.command_mode {
    " | Press / for commands (:help)".to_string()
} else { String::new() };
let status_text = format!("{} | ... |{}{}", connection_status, ..., uptime_status, hint);
```

### G3: Uptime Tracking (state.rs + status.rs + app.rs)
* state.rs:
  ```rust
  pub startup_time: Instant,
  ```
  Init: `let mut state = TuiState { startup_time: Instant::now(), ..Default::default() };`
* status.rs:
  ```rust
  let elapsed = state.startup_time.elapsed();
  let mins = (elapsed.as_secs() / 60) as u64;
  let secs = elapsed.as_secs() % 60;
  let uptime_status = format!("Uptime: {:02}:{:02}", mins, secs);
  ```
* Reducer: No action needed (tick updates via update_state_from_shared).

### G4: Clippy Fixes
* Run `cargo clippy -p avix-cli --fix --allow-dirty --allow-staged`.
* Manual: Remove #[allow(dead_code)], fix unused.

### Error Handling
* Dispatch errors → Notification::SysAlert.
* Invalid pid → parser already errs.

## User/Dev Experience
* UX: `:kill 123` → agent vanishes (AgentExit event), discoverable via `/ :help`, precise uptime, clean status.
* Flows unchanged; hints reduce magic-key reliance.
* Dev: `cargo test tui/` + manual `avix tui` → connect, spawn, kill, watch uptime.
* Keyboard: Unchanged.

## Risks & Trade-offs
* Risk: send_signal SIGKILL kills wrong pid → Mitigation: u64 parse + list_agents verify exists (optional future).
* Risk: Uptime drift → Tick 100ms fine-grained.
* Trade-off: Manual fmt vs chrono (no dep bloat).
* Risk: Clippy breaks tests → Run full suite post-fix.

## Dependencies & Prerequisites
* PROJECT-TUI-001 complete (current state).

## Success Criteria
* Functional: `:kill` dispatches SIGKILL, agent exits; hint/uptime render; clippy clean.
* Tests: +Unit for dispatch_kill, uptime fmt; coverage >95%.
* UX: Usability-agent: \"Hints intuitive, no stubs\".
* Manual: Full cycle connect-spawn-kill-logs; resize/uptime ticks.
* Regress: All TUI-001 tests pass.

## References
* TUI-001: docs/spec/PROJECT-TUI-001-spec.md, docs/dev_plans/PROJECT-TUI-001-dev-plan.md.
* Code: crates/avix-cli/src/tui/{app.rs,state.rs,parser.rs}, widgets/status.rs.
* commands.rs: send_signal exists.
* architecture/tui.md#known-gaps, 00-overview.md invariants.
* PM-REPORT-PROJECT-TUI-002-INIT-20241027.md.