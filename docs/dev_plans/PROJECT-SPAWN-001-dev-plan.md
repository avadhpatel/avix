# PROJECT-SPAWN-001-dev-plan.md: Implementation Plan for Full Agent Spawn Workflow

## Overview
Break spec into sequential gaps: TUI form validation+ATP, gateway ATP handler, kernel/proc/spawn full impl (fork/exec), kernel/proc/list, daemon persistence (agents.yaml). TDD: failing tests first. Builds on fs-gap-B (/proc/ writes). Est. 15-20 gaps, 3-5 days.

## What to Implement (Sequential Tasks)
1. TUI form validation (name/goal) + ATP Spawn cmd dispatch (avix-cli/tui).
2. ATP Command::Spawn parser + Event::AgentSpawned/Output/Failed (avix-core/atp).
3. Gateway handler: ATP spawn → IPC kernel/proc/spawn.
4. kernel/proc/spawn: PID alloc, CapToken mint, /proc/ writes, persist agents.yaml, fork/exec RuntimeExecutor.
5. RuntimeExecutor env parse + Category 2 tool register (ipc.tool-add).
6. kernel/proc/list: scan process table → Vec&lt;AgentSummary&gt;.
7. Agent output routing: RuntimeExecutor stdout → ATP Event::AgentOutput via gateway.
8. Daemon boot Phase 3: load agents.yaml → re-adopt alive pids (write /proc/, SIGSTART).
9. Agent exit: kernel cleanup (remove agents.yaml, ipc.tool-remove).
10. GC task: kernel cron stale pids.
11. TUI integration: form submit → list refresh → output tail.
12. E2E tests + manual daemon restart.

## TDD Approach
* Per-task: Write failing test → min impl → refactor.
* Examples:
  * Task 1: `#[test] fn tui_form_valid_name() { assert_err(empty); assert_ok(\"foo\"); }`
  * Task 4: `#[test] fn kernel_spawn_execs_re() { mock_exec(); let pid = spawn(..); assert_proc_files(pid); }`
  * Task 8: `#[test] fn daemon_re_adopt() { write_agents_yaml(); boot(); assert_sigstart_sent(); }`
* Integration: avix-tests-integration ATP spawn e2e.

## Detailed Implementation Guidance
* **Crates/Files**:
  | Task | Crates/Files | Key Functions/Structs |
  |------|--------------|-----------------------|
  | 1 | avix-cli/src/tui/form.rs | validate_name(goal), atp_client.spawn(name, goal) |
  | 2 | avix-core/src/atp/mod.rs | #[derive(AtpCommand)] Spawn {name, goal} |
  | 3 | avix-core/src/services/gateway/handlers.rs | handle_spawn(ctx) → ipc_call(kernel__proc__spawn) |
  | 4 | avix-core/src/kernel/proc.rs | spawn(name, goal) → u64; write_agents_yaml(); Command::new(\"./target/debug/avix-re\") |
  | 5 | avix-core/src/runtime_executor/main.rs | parse_env_pid_token_goal(); runtime_executor::register_cat2_tools(); |
  | 6 | avix-core/src/kernel/proc.rs | list() → Vec&lt;Summary&gt;; process_table.iter() |
  | 7 | avix-core/src/runtime_executor/turn_loop.rs | tracing::info!(output=%chunk); atp_event(AgentOutput {pid, chunk}) |
  | 8 | avix-core/src/kernel/boot.rs | phase3_re_adopt(); for agent in load_yaml(); if alive(pid) { rewrite_proc(pid); send_sigstart(pid); } |
  | 9 | avix-core/src/kernel/proc.rs | on_exit(pid) { remove_yaml(pid); ipc.tool-remove(cat2); } |
  | 11 | avix-cli/src/tui/app.rs | on Event::AgentSpawned → state.agents.insert(); poll kernel/proc/list |

* **Tracing/Logging**: span!(\"spawn.pid={} name={} \", pid, name); debug!(\"re-adopted pid={}\"); info!(\"agent {} output: {}\", pid, chunk);
* **Errors**: AvixError::SpawnFailed(reason); propagate ?; ATP Event::Error.
* **Invariants (CLAUDE.md)**: Fresh IPC per spawn/list, no unwrap (handle exec fail), tokio::spawn gc task, Arc&lt;ProcessTable&gt; RwLock.
* **Binary**: Ensure avix-re (RuntimeExecutor) built (cargo build --bin avix-re).

## Testing Requirements
* Unit: spawn parser, validation, yaml rw, pid alive check.
* Integration: ATP→IPC→mock_exec spawn (no real fork).
* Manual: avix tui spawn → ps aux | grep avix-re → kill daemon → restart → ps shows + list tool.
* Edge: invalid form, spawn fail (bad binary), re-adopt dead pid (gc), quota.
* Coverage: &gt;95% kernel/proc/*.

## Usability Considerations
* TUI form: intuitive Tab/Enter, error inline.
* List: auto-refresh 1s, select → tail output.
* Post-impl: usability-agent: CLI journeys, form friction, reconnect seamless.

## Estimated Effort & Priority
* Effort: High (daemon state, fork/exec, persistence). ~30h code/test + 8h debug/restart races.
* Priority: Critical — core OS process model.

## Feedback Integration
* From testing-agent: stub gaps confirmed.
* No prior coding/usability. Update post-cycle.

## Completion Checklist
- [ ] Tasks 1-12 done, cargo test --workspace
- [ ] Clippy/fmt clean
- [ ] E2E: tui spawn + daemon restart manual pass
- [ ] Coverage &gt;95%
- [ ] testing-agent: observability OK
- [ ] usability-agent: UX APPROVED
- [ ] Hand-off: \"Spawn complete, ready for docs/program-manager.\"