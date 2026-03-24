# TUI Completion Gaps (post G/H implementation)

Current status: 80% — core loop/events/state/modal work. Stubs in ui fn.

## Missing UI Components (implement widgets + integrate in app.rs ui fn)

1. **Agent List Widget** (priority high)
   - `widgets/agent_list.rs`
   - List<Block> from `state.agents` (pid/name/status/goal).
   - Selectable, scrollable.
   - Test: render 3 agents, select.

2. **Notification Bar/Popup** (high)
   - `widgets/notification_bar.rs`
   - Compact bar or popup list unread notifs (kind/message).
   - Toggle on state.notifications_popup_open.
   - Mark read on select.
   - Test: render 2 unread.

3. **New Agent Form** (medium)
   - `widgets/new_agent_form.rs`
   - Input fields for name/goal (focused cursor).
   - Submit → spawn_agent.
   - Test: focus switch, submit stub.

4. **Agent Output Pane** (high)
   - Integrate AgentOutputBuffer render in ui (per selected agent?).
   - Scroll/viewport, scroll_up/down/bottom.
   - Test: push lines, visible_lines.

## Layout Updates (app.rs ui fn)
- Vertical: status | agents list | output pane | notifs bar.
- Tabs/split for agents/output/notifs/form.
- HilModal overlay.

## Tests
- widgets/ * .rs: #[tokio::test] render_snapshot (assert_snapshot).
- app.rs: key handling full (spawn success, HIL resolve).

## Verification
- `avix --tui`: Full interactive (spawn from form, view output, approve HIL, mark notifs).
- Clippy/fmt/test clean.

Est: 1 agent task.