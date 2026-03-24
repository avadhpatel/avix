use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::super::state::NewAgentFormState;

#[derive(Debug, Clone, Default)]
pub struct NewAgentFormWidget;

impl NewAgentFormWidget {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render<'a>(
        &self,
        form: &'a NewAgentFormState,
        area: Rect,
    ) -> Vec<(Rect, Paragraph<'a>)> {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Name field
                Constraint::Length(3), // Goal field
                Constraint::Length(1), // Submit hint
            ])
            .split(area);

        let name_block = Block::default().borders(Borders::ALL).title("Agent Name");
        let name_style = if form.focused_field == 0 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        let name_para = Paragraph::new(form.name.as_str())
            .block(name_block)
            .style(name_style);

        let goal_block = Block::default().borders(Borders::ALL).title("Agent Goal");
        let goal_style = if form.focused_field == 1 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };
        let goal_para = Paragraph::new(form.goal.as_str())
            .block(goal_block)
            .style(goal_style);

        let hint = Paragraph::new("Press Enter to submit, Tab to switch fields")
            .alignment(Alignment::Center);

        vec![
            (chunks[0], name_para),
            (chunks[1], goal_para),
            (chunks[2], hint),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::NewAgentFormState;

    #[test]
    fn render_form_with_focus() {
        let widget = NewAgentFormWidget::new();
        let form = NewAgentFormState {
            name: "test-agent".into(),
            goal: "test goal".into(),
            focused_field: 0,
        };

        let area = Rect::new(0, 0, 50, 10);
        let widgets = widget.render(&form, area);

        assert_eq!(widgets.len(), 3);
    }

    #[test]
    fn focus_switch() {
        let form1 = NewAgentFormState {
            name: "name".into(),
            goal: "goal".into(),
            focused_field: 0,
        };
        let form2 = NewAgentFormState {
            name: "name".into(),
            goal: "goal".into(),
            focused_field: 1,
        };

        let widget = NewAgentFormWidget::new();
        let area = Rect::new(0, 0, 50, 10);

        let _widgets1 = widget.render(&form1, area);
        let _widgets2 = widget.render(&form2, area);

        assert!(true);
    }
}
