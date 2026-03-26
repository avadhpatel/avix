use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

/// HelpModalWidget renders a help modal with command descriptions.
#[derive(Debug, Clone, Default)]
pub struct HelpModalWidget;

#[allow(dead_code)]
impl HelpModalWidget {
    pub fn new() -> Self {
        Self
    }

    /// Render the help modal.
    /// Returns a List widget.
    pub fn render(&self, _area: Rect) -> List<'_> {
        let items = vec![
            ListItem::new("Global Commands:"),
            ListItem::new("  /connect, /c     - Connect to the server"),
            ListItem::new("  /quit, /q        - Quit the application"),
            ListItem::new("  /help, /h, /?    - Show this help"),
            ListItem::new(""),
            ListItem::new("Agent Commands:"),
            ListItem::new("  /spawn <name> <goal> - Spawn a new agent"),
            ListItem::new("  /kill <pid>      - Kill an agent by PID"),
            ListItem::new(""),
            ListItem::new("View Commands:"),
            ListItem::new("  /logs, /log      - Toggle event log pane"),
            ListItem::new("  /notifs, /n      - Toggle notifications popup"),
            ListItem::new("  /new-agent-form, /f - Toggle new agent form"),
            ListItem::new(""),
            ListItem::new("Hotkeys:"),
            ListItem::new("  /                - Enter command mode"),
            ListItem::new("  Esc              - Exit current mode"),
            ListItem::new("  ↑↓               - Navigate lists"),
            ListItem::new("  Enter            - Select/Submit"),
            ListItem::new("  Tab              - Switch fields (in forms)"),
            ListItem::new(""),
            ListItem::new("Magic Keys (backward compatible):"),
            ListItem::new("  q                - Quit"),
            ListItem::new("  c                - Connect"),
            ListItem::new("  a                - Spawn test agent"),
            ListItem::new("  f                - Toggle new agent form"),
            ListItem::new("  n                - Toggle notifications"),
        ];

        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Help - Press Esc to close")
                    .title_alignment(Alignment::Center),
            )
            .highlight_style(Style::default().fg(Color::Yellow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_help_modal() {
        let widget = HelpModalWidget::new();
        let list = widget.render(Rect::new(0, 0, 80, 24));
        assert!(list.len() > 10);
        // TODO: check content when List exposes items
    }
}
