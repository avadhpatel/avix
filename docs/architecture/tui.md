# Avix TUI Dashboard (PROJECT-TUI-001)

## Overview

The TUI provides a real-time dashboard for agent management, event monitoring, notifications, and HIL approvals. Launched via `avix tui`.

Built with [ratatui](https://ratatui.rs/) (crossterm backend). Integrates `avix-client-core` ATP client for live events.

Key features:
* Agent list with status/output tailing
* Command mode (`/`) with parser (`:spawn`, `:kill`, etc.)
* Event log (recent ATP events/commands)
* Persistent notifications (load/save)
* Fullscreen modals: HIL approve/deny, new agent form, notifs/help popups

## Layout

Vertical layout:
* Status bar (3 lines): connection, unread notifs/HIL pending, cmd mode indicator
* Agents list (20% height): ↑↓ select, pid/name/status/goal
* Main pane (min 10 lines):
  * Left: event log (30%, toggle with `:logs`)
  * Right: selected agent stdout (buffered, scrollable)
* Command input (2 lines, when `/` active)
* Notifications bar (1 line): unread count, 'n' toggle popup

Modals/popups overlay fullscreen or centered.

## Command Mode

* Enter: `/`
* Edit: char/backspace/left/right/↑↓ history
* Submit: Enter (parses `:input`)
* Exit: Esc

## Parser (`tui/parser.rs`)

Parses `:command [args]` with quoted strings (`\"multi word\"`).

Supported:
* `:quit`/`:q` — exit
* `:connect`/`:c` — connect ATP WS
* `:spawn &lt;name&gt; &quot;&lt;goal&gt;&quot;` — spawn agent
* `:kill &lt;pid&gt;` — kill agent (P2 stub)
* `:help`/`:h`/`:?` — help modal
* `:logs`/`:log` — toggle event log
* `:notifs`/`:n` — notifs popup
* `:new-agent-form`/`:new`/`:f` — new agent form

Invalid → error notification.

Tests cover all cases + quoting.

## Event Log

Ring buffer (last 10):
* Sent commands (`:spawn foo &quot;goal&quot;`)
* ATP events (kind/pid/summary: AgentOutput pid=123)

Toggle visibility left/right split.

## ATP Integration

`app.rs` dispatch loop:
* Connect → subscribe events → spawn background handler
* Events → update state/output/notifs/HIL modal
* Commands → ATP calls (spawn_agent, etc.)

State sync every 100ms: list_agents → agents vec.

## Key Bindings (Normal Mode)

| Key | Action |
|-----|--------|
| q | Quit |
| c | Connect (if disconnected) |
| / | Enter command mode |
| ↑↓ | Select agent |
| a | Spawn test agent |
| f | Toggle new agent form |
| n | Toggle notifications popup |
| l | Toggle logs (?) |

**HIL Modal**: a=approve, d=deny, Esc=dismiss  
**New Agent Form**: Tab=field, chars=input, Enter=spawn, Esc=cancel  
**Cmd Mode**: ↑↓=history, ←→=cursor  
**Notifs Popup**: ↑↓=select, Enter=read, Esc=close

## Known Gaps (Usability P2-P4)
* :kill stub (P2)
* / hint in status (P3)
* Uptime in status (P4)

## Implementation Notes
* Reducer pattern (Action enum → TuiState)
* Async non-blocking (tokio mpsc for actions)
* Persisted notifs (JSON)
* Buffered output per PID (scroll view)

See `crates/avix-cli/src/tui/` for source.
