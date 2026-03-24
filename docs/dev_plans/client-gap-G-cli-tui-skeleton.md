# Client Gap G — `avix-cli` Ratatui TUI Skeleton

> **Status:** Pending
> **Priority:** Medium
> **Depends on:** Client gap F (ATP subcommands + global opts)
> **Blocks:** Client gap H (TUI agent streaming + HIL modal)
> **Affects:** `crates/avix-cli/Cargo.toml`,
>   `crates/avix-cli/src/tui/` (new module tree)

---

## Problem

The CLI has no interactive TUI mode. Users who want a live dashboard must currently
use external tools or the GUI. A Ratatui-based TUI provides the same multi-agent
dashboard experience in the terminal without a desktop environment.

---

## Scope

Build the Ratatui TUI skeleton: layout, navigation, key bindings, and static placeholder
content. No live ATP data yet (gap H wires events). The TUI must draw correctly, respond
to key input, and quit cleanly. All layout logic must be unit-testable without a real
terminal.

---

## When TUI Mode Is Active

`avix` without a subcommand (or `avix tui`) launches TUI mode if stdout is a TTY.
If stdout is not a TTY (pipe / CI), fall back to `avix --help`.

```rust
fn main() -> Result<()> {
    // If no subcommand given and stdout is a tty → launch TUI
    if no_subcommand && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return tui::run().await;
    }
    // else: existing clap dispatch
}
```

---

## Module Tree

```
crates/avix-cli/src/tui/
├── mod.rs         ← pub fn run() -> Result<()>; init terminal, event loop
├── app.rs         ← TuiApp struct (state machine)
├── layout.rs      ← compute_layout() → pure function, testable
├── widgets/
│   ├── mod.rs
│   ├── sidebar.rs ← Sidebar widget (agent list)
│   └── agent_panel.rs ← AgentPanel widget (scrollable output)
└── keys.rs        ← key binding constants + dispatch
```

---

## Layout Specification

```
┌─────────────────────────────────────────────────────────────────┐
│  avix v0.x  ·  Connected to 127.0.0.1:7700  ·  sess-abc         │ header (1 line)
├──────────────────┬──────────────────────────────────────────────┤
│  [+ Add Agent]   │  [Agent Tab 1]  [Agent Tab 2]  …             │ tab bar (1 line)
│ ─────────────── │ ───────────────────────────────────────────── │
│  Active (2)      │                                               │
│  > researcher    │  researcher – summarise Q3 report             │
│    analyst       │  running · pid 42 · turn 3                    │
│ ─────────────── │ ─────────────────────────────────────────────  │
│  Recent (0)      │  [scrollable agent output here]               │
│                  │                                               │
│ ─────────────── │                                               │
│  System          │                                               │
│    CPU: 12%      │                                               │
│    Mem: 44%      │                                               │
├──────────────────┴──────────────────────────────────────────────┤
│  🔔 0  ·  q=quit  Ctrl+N=new  j/k=nav  Enter=select  ?=help     │ status bar (1 line)
└─────────────────────────────────────────────────────────────────┘
```

The sidebar is fixed-width (22 columns). The main area takes the remaining width.

---

## `tui/app.rs` — `TuiApp`

```rust
use avix_client_core::state::SharedState;

pub struct TuiApp {
    pub state: SharedState,
    pub selected_sidebar_idx: usize,
    pub selected_tab_idx: usize,
    pub sidebar_focus: bool,
    pub should_quit: bool,
    pub scroll_offset: u16,
}

impl TuiApp {
    pub fn new(state: SharedState) -> Self { … }

    /// Handle a key event. Returns true if the event was consumed.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool { … }

    /// Returns the PID of the currently focused agent, if any.
    pub fn focused_pid(&self) -> Option<u64> { … }
}
```

---

## `tui/keys.rs` — Key Bindings

| Key | Action |
|-----|--------|
| `q` | Quit |
| `Ctrl+N` | Open "new agent" form (gap H) |
| `j` / `↓` | Move sidebar selection down |
| `k` / `↑` | Move sidebar selection up |
| `Enter` | Focus selected agent's panel |
| `Tab` | Cycle agent tabs (main area) |
| `Shift+Tab` | Cycle agent tabs backward |
| `g` / `Home` | Scroll to top of output |
| `G` / `End` | Scroll to bottom of output |
| `PageUp` / `PageDown` | Scroll output |
| `?` | Toggle help overlay |

---

## `tui/layout.rs` — `compute_layout` (pure function)

```rust
use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct TuiLayout {
    pub header: Rect,
    pub tab_bar: Rect,
    pub sidebar: Rect,
    pub main: Rect,
    pub status_bar: Rect,
}

/// Compute layout from a terminal Rect. Pure — no side effects.
pub fn compute_layout(area: Rect) -> TuiLayout { … }
```

The layout splits:
1. Vertical: header(1) / body(fill) / status_bar(1)
2. Body horizontal: sidebar(22) / main(fill)
3. Main vertical: tab_bar(1) / content(fill)

---

## `tui/mod.rs` — Event Loop

```rust
pub async fn run() -> anyhow::Result<()> {
    // 1. Build ClientConfig from file / env
    // 2. SharedState = new_shared(config)
    // 3. Attempt ATP connect (non-blocking — show "Connecting…" in header if pending)
    // 4. Init crossterm raw mode + alternate screen
    // 5. Loop:
    //    a. Draw frame: header / sidebar / main / status
    //    b. Poll crossterm events with 16ms timeout
    //    c. Handle KeyEvent via app.handle_key()
    //    d. Handle resize events
    //    e. Break on app.should_quit
    // 6. Restore terminal
}
```

---

## Tests

All tests exercise `app.rs` and `layout.rs` — no real terminal needed.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    // layout.rs
    #[test]
    fn compute_layout_header_is_1_line() {
        let area = Rect { x: 0, y: 0, width: 120, height: 40 };
        let layout = compute_layout(area);
        assert_eq!(layout.header.height, 1);
    }

    #[test]
    fn compute_layout_status_bar_is_1_line() {
        let area = Rect { x: 0, y: 0, width: 120, height: 40 };
        let layout = compute_layout(area);
        assert_eq!(layout.status_bar.height, 1);
    }

    #[test]
    fn compute_layout_sidebar_is_22_wide() {
        let area = Rect { x: 0, y: 0, width: 120, height: 40 };
        let layout = compute_layout(area);
        assert_eq!(layout.sidebar.width, 22);
    }

    #[test]
    fn compute_layout_main_fills_remainder() {
        let area = Rect { x: 0, y: 0, width: 120, height: 40 };
        let layout = compute_layout(area);
        assert_eq!(layout.main.width, 120 - 22);
    }

    // app.rs
    #[test]
    fn handle_key_q_sets_should_quit() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let state = avix_client_core::state::new_shared(Default::default());
        let mut app = TuiApp::new(state);
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        app.handle_key(key);
        assert!(app.should_quit);
    }

    #[test]
    fn handle_key_j_increments_sidebar_idx() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let state = avix_client_core::state::new_shared(Default::default());
        let mut app = TuiApp::new(state);
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(key);
        assert_eq!(app.selected_sidebar_idx, 1);
    }

    #[test]
    fn handle_key_k_doesnt_underflow() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let state = avix_client_core::state::new_shared(Default::default());
        let mut app = TuiApp::new(state);
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        app.handle_key(key);
        assert_eq!(app.selected_sidebar_idx, 0);  // saturates at 0
    }

    #[test]
    fn focused_pid_returns_none_when_no_agents() {
        let state = avix_client_core::state::new_shared(Default::default());
        let app = TuiApp::new(state);
        assert!(app.focused_pid().is_none());
    }
}
```

---

## Dependencies to add to `avix-cli/Cargo.toml`

```toml
ratatui    = "0.29"
crossterm  = { version = "0.28", features = ["event-stream"] }
```

---

## Success Criteria

- [ ] `avix` (no subcommand, TTY) launches TUI without panicking
- [ ] Layout renders header / sidebar / main / status bar at 80×24 and 120×40
- [ ] `q` quits cleanly, terminal is restored
- [ ] `j` / `k` move sidebar selection; selection saturates at boundaries
- [ ] `?` toggles help overlay (can be a static text paragraph for now)
- [ ] All layout and key-handling tests pass
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` — zero warnings
