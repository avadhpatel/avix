# Client Gap H — `avix-cli` TUI Live Agent Streaming, HIL Modal + Notifications

> **Status:** Pending
> **Priority:** Medium
> **Depends on:** Client gaps C (EventEmitter), D (NotificationStore), G (TUI skeleton)
> **Blocks:** nothing (this completes the CLI TUI)
> **Affects:** `crates/avix-cli/src/tui/` (all modules extended)

---

## Problem

The TUI skeleton (gap G) shows static placeholder content. Gap H wires live ATP events
to the TUI: agent output streams into scrollable panels, agent status changes reflect in
the sidebar, HIL requests trigger a full-screen modal, and notifications appear in the
status bar.

---

## Scope

Wire `EventEmitter` events into `TuiApp` state. Add scrollable `AgentOutputWidget`.
Implement HIL full-screen modal. Add notification badge + popup list. Add "new agent"
form. All UI state is driven by the `NotificationStore` and `AppState` from
`avix-client-core`.

---

## What Needs to Be Built

### 1. Background event → state bridge

In `tui/mod.rs`, after ATP connect, start a background task that reads from
`EventEmitter` and mutates `SharedState`:

```rust
tokio::spawn(async move {
    let mut rx = emitter.subscribe_all();
    loop {
        match rx.recv().await {
            Ok(event) => dispatch_event(&state, &notifications, event).await,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("TUI event receiver lagged {n} messages");
            }
            Err(_) => break,
        }
    }
});
```

`dispatch_event` matches on `EventKind`:
- `AgentOutput` → append text to `agent_output_buffer[pid]`
- `AgentStatus` → update `AppState.agents[pid].status`
- `AgentExit` → mark agent stopped + add `NotificationStore` entry
- `HilRequest` → add HIL notification + set `TuiApp.pending_hil`
- `HilResolved` → resolve in `NotificationStore`
- `SysAlert` → add notification

The event loop must use a `tokio::sync::mpsc` channel (not direct state mutation from
the background task) to avoid holding a write lock while drawing. The draw loop drains
the channel on each tick before rendering.

---

### 2. `AgentOutputBuffer`

```rust
use std::collections::VecDeque;

pub struct AgentOutputBuffer {
    /// Circular buffer — keeps the last MAX_LINES lines
    lines: VecDeque<String>,
    pub scroll_offset: u16,
}

const MAX_LINES: usize = 5000;

impl AgentOutputBuffer {
    pub fn push_text(&mut self, text: &str) { … }
    pub fn visible_lines(&self, height: u16) -> Vec<&str> { … }
    pub fn scroll_down(&mut self, n: u16) { … }
    pub fn scroll_up(&mut self, n: u16) { … }
    pub fn scroll_to_bottom(&mut self) { … }
}
```

---

### 3. HIL modal (`tui/widgets/hil_modal.rs`)

When `TuiApp.pending_hil` is `Some(HilState)`, the draw loop renders a full-screen
modal overlay instead of the normal layout:

```
╔═══════════════════════════════════════════════════╗
║  ⚠  Human Input Required                          ║
║                                                   ║
║  Agent: researcher (PID 42)                       ║
║  Request: "Send the email to board@example.com?"  ║
║                                                   ║
║  Timeout: 8m 42s remaining                       ║
║                                                   ║
║  [A] Approve   [D] Deny   [N] Add note            ║
╚═══════════════════════════════════════════════════╝
```

Key bindings inside modal:
- `A` / `a` → `commands::resolve_hil(…, approved: true)`
- `D` / `d` → `commands::resolve_hil(…, approved: false)`
- `Esc` → dismiss modal without responding (HIL stays pending)

After approve/deny, call `notification_store.resolve_hil(…)` and clear `pending_hil`.

Countdown: use `Instant::now()` recorded when the HIL arrived and `timeout_secs` to
compute remaining time. The draw loop re-renders every 500ms when a HIL is pending so
the countdown ticks.

---

### 4. Notification bar + popup (`tui/widgets/notification_bar.rs`)

**Status bar** (always visible, bottom line):

```
🔔 3 unread  ·  [last notification summary]  ·  q=quit  …
```

**Notification popup** (toggle with `N` key):

```
┌── Notifications (3 unread) ──────────────────────────────┐
│ [HIL]  PID 42 – "Send email to board?"    2m ago  PENDING │
│ [EXIT] PID 17 – analyst exited (code 0)   5m ago          │
│ [ALRT] disk space low on /                8m ago          │
│                                                           │
│  j/k = navigate  Enter = jump to agent  Esc = close       │
└───────────────────────────────────────────────────────────┘
```

The popup renders as a floating layer above the main layout using `ratatui::widgets::Clear`.

---

### 5. "New agent" form (`tui/widgets/new_agent_form.rs`)

Triggered by `Ctrl+N`. A small centered modal with two fields:

```
┌── New Agent ──────────────────┐
│ Name:  [researcher           ]│
│ Goal:  [summarise Q3 report  ]│
│                               │
│  Enter=spawn  Esc=cancel      │
└───────────────────────────────┘
```

On `Enter`: call `commands::spawn_agent(…)` and dismiss form.
On success: the `AgentStatus` event from the server will add the agent to the sidebar
automatically via the event bridge.

---

## Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // AgentOutputBuffer
    #[test]
    fn push_text_splits_on_newlines() {
        let mut buf = AgentOutputBuffer::default();
        buf.push_text("line1\nline2\nline3");
        assert_eq!(buf.lines.len(), 3);
    }

    #[test]
    fn push_text_respects_max_lines() {
        let mut buf = AgentOutputBuffer::default();
        for i in 0..MAX_LINES + 10 {
            buf.push_text(&format!("line {i}\n"));
        }
        assert_eq!(buf.lines.len(), MAX_LINES);
    }

    #[test]
    fn scroll_to_bottom_shows_last_line() {
        let mut buf = AgentOutputBuffer::default();
        for i in 0..100 { buf.push_text(&format!("line {i}\n")); }
        buf.scroll_to_bottom();
        let visible = buf.visible_lines(10);
        assert_eq!(visible.last().unwrap(), &"line 99");
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut buf = AgentOutputBuffer::default();
        buf.push_text("only one line\n");
        buf.scroll_up(100);
        assert_eq!(buf.scroll_offset, 0);
    }

    // HIL modal state
    #[test]
    fn hil_countdown_secs_remaining() {
        use std::time::{Duration, Instant};
        let arrived = Instant::now() - Duration::from_secs(30);
        let timeout = 600u32;
        let remaining = timeout.saturating_sub(arrived.elapsed().as_secs() as u32);
        assert!(remaining >= 569 && remaining <= 571);
    }

    // dispatch_event routing (unit — no real channel)
    #[tokio::test]
    async fn dispatch_agent_output_adds_to_buffer() {
        // Build a fake SharedState with one agent
        // Dispatch AgentOutput event for that agent's pid
        // Assert buffer has the text line
    }

    #[tokio::test]
    async fn dispatch_hil_request_sets_pending_hil() {
        // Dispatch HilRequest event
        // Assert TuiApp.pending_hil is Some(…)
    }

    #[tokio::test]
    async fn dispatch_hil_resolved_clears_pending_hil() {
        // Set TuiApp.pending_hil = Some(…)
        // Dispatch HilResolved event with matching hil_id
        // Assert TuiApp.pending_hil is None
    }
}
```

---

## Success Criteria

- [ ] `agent.output` events stream into the correct agent's scrollable panel in real time
- [ ] Agent status changes (paused/stopped/crashed) reflect in the sidebar
- [ ] HIL modal appears on `hil.request`, shows countdown, accepts A/D keys
- [ ] Approving/denying HIL calls `commands::resolve_hil` and dismisses modal
- [ ] Notification badge count is accurate; popup lists all notifications newest-first
- [ ] `Ctrl+N` form spawns an agent and it appears in the sidebar on `agent.status` event
- [ ] `MAX_LINES` cap prevents unbounded memory growth in output buffers
- [ ] All tests pass; `cargo test --workspace` green
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
