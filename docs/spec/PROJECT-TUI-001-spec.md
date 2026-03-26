# PROJECT-TUI-001-spec.md: TUI Improvements for Command Input, Discovery, Status, and Hotkey Navigation

## Version
1.0 (Initial Draft) - October 2024

## Motivation & Problem Statement
The current TUI implementation in `crates/avix-cli/src/tui/` relies on undocumented \"magic keys\" (e.g., 'q' to quit, 'c' to connect, 'a' to spawn test agent, 'f' for new agent form, 'n' for notifications). This approach has several pain points:
* **Poor discoverability**: Users have no way to learn available actions without external documentation or trial-and-error.
* **Limited input**: No support for structured commands with arguments (e.g., spawning an agent with a custom name and goal like `spawn researcher \"analyze logs\"`).
* **Opaque status**: Connection status is minimal; no visibility into recent events, sent/received ATP messages, or background activity.
* **No command history or logs**: Users cannot review recent actions or ATP events (outputs, status changes, HIL requests).
* **Static layout**: Fixed heights (e.g., agents list at 10 lines) do not adapt well to different terminal sizes.

These issues hinder keyboard-driven UX, especially for power users expecting vi-like discoverability (`/` for search/cmd mode, `:` for ex commands). Aligns with Avix CLI-first philosophy and CLAUDE.md emphasis on observability (tracing everywhere).

## Goals
* Introduce **structured command input** via hotkey `/` entering a command mode with `:`-prefixed commands (e.g., `:spawn foo \"goal\"`, `:connect`, `:quit`).
* **Command discovery**: `:help` or `?` displays a modal with categorized command list, descriptions, and hotkey aliases.
* **Enhanced status pane**: Expanded top bar showing connection status, agent counts (total/running), unread notifications/HIL pending, session uptime.
* **Event log pane**: Real-time tail of last 10 TUI events (sent commands + received ATP events) in a toggleable pane (`:logs`).
* **Responsive layout**: Adaptive splits (status 3 lines, agents 20%, log 20%, output 60%, command bar 2 lines).
* **Backward compatibility**: Existing modals (HIL, new agent form, notifications) and magic keys remain functional as aliases (e.g., 'q' == `:q`).
* **Keyboard-driven UX**: Arrow keys for navigation, vi-inspired cmd mode for actions.

## Non-Goals
* Full vi/vim emulation (no insert/normal/visual modes, no hjkl by default — keep arrows).
* Mouse support or touch gestures.
* Configurable keybindings or themes (future PROJECT-TUI-002).
* Persistent command history across sessions (session-only circular buffer).
* ATP command logging persisted to disk (local TUI state only).
* GUI porting (TUI-focused).

## Architecture Impact
Purely client-side in `crates/avix-cli/src/tui/` — no changes to `avix-core`, ATP protocol, IPC, HIL invariants, or server-side (per CLAUDE.md).
* **State**: Extend `TuiState` with `command_mode`, `CommandInputState`, `EventLog`.
* **Actions**: New variants for input handling, command submission, logging.
* **Widgets**: New `CommandBarWidget`, `EventLogWidget`, `ExpandedStatusWidget`; update layout in `ui()`.
* **Event Flow**: `dispatch_event()` → `Action::LogEvent`; command submit → log + ATP dispatch via existing `commands.rs`.
* **Observability**: Add `tracing::debug!` spans for all new paths (e.g., `debug!(\"cmd=parse: {:?}\", parsed)`).
* **Performance**: Event log capped at 10; no blocking ops. Poll rate unchanged (100ms).
* **Layout**: Ratatui `Layout` with `Constraint::Percentage` for responsiveness.

No impact on core invariants (e.g., fresh IPC per call, llm.svc mediation).

## Detailed Design

### Data Structures (in `state.rs`)
```rust
#[derive(Clone, Default)]
pub struct CommandInputState {
    pub input: String,           // e.g., \":spawn foo \\\"goal\\\"\"
    pub cursor_pos: usize,       // For left/right arrow, backspace
    pub history: Vec<String>,    // Session history, up/down nav
    pub history_index: usize,
}

#[derive(Clone)]
pub enum TuiEvent {
    SentCommand { cmd: String, timestamp: Instant },
    ReceivedAtp { kind: EventKind, pid: Option<u64>, summary: String, timestamp: Instant },
}

#[derive(Clone, Default)]
pub struct EventLog {
    events: VecDeque<TuiEvent>,  // Fixed max 10
}

#[derive(Clone)]
pub enum ParsedCommand {
    Quit,
    Connect,
    Spawn { name: String, goal: String },
    Kill { pid: u64 },
    Help,
    ToggleLogs,
    ToggleNotifications,
    ToggleNewAgentForm,
    // Extensible...
    Invalid(String),
}

impl TuiState {
    // Add fields:
    pub command_mode: bool,
    pub command_input: Option<CommandInputState>,
    pub event_log: EventLog,
    pub log_visible: bool,
}
```

New `Action` variants:
```rust
Action::EnterCommandMode,
Action::ExitCommandMode,
Action::UpdateCommandInput { delta: InputDelta },  // Char(c), Backspace, Left, Right, HistoryUp, HistoryDown
Action::SubmitCommand(String),
Action::LogEvent(TuiEvent),
Action::ToggleLogs,
```

### Command Parser (new `parser.rs`)
* Trigger: input starts with `:`.
* Split on spaces, support quoted strings (`\"goal with space\"`).
* Parse pid as `u64`.
* Examples:
  * `:spawn foo \"observe X\"` → `Spawn {name: \"foo\", goal: \"observe X\"}`
  * `:kill 123` → `Kill {pid: 123}`
  * `:q` → `Quit`
* Magic key aliases: 'q' → `Quit`, etc.
* Errors → `Notification::SysAlert`.

### Widget Updates (new files in `widgets/`)
* **CommandBarWidget**: Input field with blinking cursor (`│` or `_`), history hints.
* **EventLogWidget**: Table/List with timestamp | icon | summary (truncate to width).
  * Icons: ➤ sent, 📥 received, ⚠ HIL, etc.
* **StatusWidget**: Multi-line Paragraph:
  ```
  Connected | Agents: 3/5 | Notifs: 2 | HIL: 1 | Uptime: 00:05:23
  ```
* **HelpModalWidget**: Scrollable list of commands grouped (Global, Agent, View).

### Layout (in `app.rs::ui()`)
```rust
let layout = Layout::vertical([
    Constraint::Length(3),   // Status
    Constraint::Percentage(20), // Agents
]).split(size);

let main_area = Layout::horizontal([
    Constraint::Percentage(if state.log_visible { 30 } else { 0 }),
    Constraint::Percentage(100),
]).split(layout[2]);  // log | output

// Bottom:
if state.command_mode {
    // Command bar
}
```
Modals overlay entire frame.

### Event Handling & Integration
* **Sent logging**: Before ATP call, `action_tx.send(LogEvent(SentCommand {cmd}))`.
* **Received**: In `dispatch_event()`, format summary (e.g., \"AgentOutput pid=42: hello\"), `LogEvent(ReceivedAtp {..})`.
* **Dispatch**: Use existing `AtpClient::dispatcher`, `spawn_agent()`, `resolve_hil()`, etc.
* **Reducer**: `event_log.events.push_back(); if >10 pop_front();`
* **Key handling priority**: HIL modal > form > notifs popup > command_mode > normal.

### Error Handling
* Parse errors: `Notification::SysAlert(error)`.
* ATP failures: Existing + log.
* `anyhow::Result` in async dispatch.

## User/Dev Experience
* **UX Flows**:
  1. Startup → status \"Disconnected\" → `/` → `:connect` → agents appear, events logged.
  2. `/` → `:spawn researcher \"analyze /proc\"` → sent log → agent in list → output streams.
  3. `:help` → modal: \"Global: :connect :quit\", \"Agent: :spawn <name> <goal> :kill <pid>\".
  4. `:logs` → toggle event pane.
* **Keyboard**:
  | Normal | Cmd Mode |
  |--------|----------|
  | ↑↓ agent nav | ←→ cursor, ↑↓ history, esc exit, enter submit
  | / enter cmd | chars, backspace
  | existing modals | :help ?
* Dev: Cargo test tui/, manual `avix tui`.

## Risks & Trade-offs
* **Risk**: Terminal resize mid-render → ratatui handles via `terminal.draw`.
  * Mitigation: `Constraint::Min(1)`, test 80x24.
* **Risk**: Complex parser → keep manual split/quote, unit tests 100%.
* **Risk**: Key conflicts → esc universal exit.
* **Trade-off**: Fixed 10 events vs infinite → fixed for perf/perf.
* **Risk**: ATP reconnect drops log → acceptable, session ephemeral.

## Dependencies & Prerequisites
* None. Builds on current TUI (app.rs ~571 lines, widgets simple lists/buffers).

## Success Criteria
* **Functional**: All spec commands parse/dispatch/log correctly (unit + e2e tests).
* **UX**: Usability-agent rates \"intuitive/discoverable, no magic needed\".
* **Perf**: No frame drops <100ms poll; responsive resize.
* **Coverage**: `cargo tarpauin tui/` >90%.
* **Regression**: Existing HIL/form/notifs unchanged.
* **Manual**: Full journey: connect-spawn-output-HIL-kill-logs-help.

## References
* Current code: `crates/avix-cli/src/tui/app.rs`, `state.rs`, `widgets/*` (lists, circular buffers).
* CLAUDE.md: Client observability, no println!, tracing::debug!.
* Ratatui 0.26 docs: layouts, input widgets, modals.
* Existing specs: `docs/spec/avix-terminal-protocol.md` (ATP context).