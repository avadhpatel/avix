use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::state::TuiState;

/// CommandBarWidget renders the command input bar at the bottom when in command mode.
#[derive(Debug, Clone, Default)]
pub struct CommandBarWidget;

#[allow(dead_code)]
impl CommandBarWidget {
    pub fn new() -> Self {
        Self
    }

    /// Render the command bar.
    /// Returns a Paragraph widget.
    pub fn render(&self, state: &TuiState) -> Paragraph<'_> {
        let text = if let Some(input_state) = &state.command_input {
            let mut display = input_state.input.clone();
            // Insert cursor as '|'
            if input_state.cursor_pos <= display.len() {
                display.insert(input_state.cursor_pos, '|');
            } else {
                display.push('|');
            }
            format!("/{}", display)
        } else {
            "".to_string()
        };

        Paragraph::new(text)
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .block(Block::default().borders(Borders::TOP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::{CommandInputState, TuiState};

    #[test]
    fn render_empty_command() {
        let mut state = TuiState::default();
        state.command_mode = true;
        state.command_input = Some(CommandInputState {
            input: "".to_string(),
            cursor_pos: 0,
            history_index: 0,
        });
        let widget = CommandBarWidget::new();
        let _para = widget.render(&state);
        // TODO: test content
    }

    #[test]
    fn render_command_with_input() {
        let mut state = TuiState::default();
        state.command_mode = true;
        state.command_input = Some(CommandInputState {
            input: "help".to_string(),
            cursor_pos: 4,
            history_index: 0,
        });
        let widget = CommandBarWidget::new();
        let _para = widget.render(&state);
        // TODO: test content
    }

    #[test]
    fn render_command_cursor_middle() {
        let mut state = TuiState::default();
        state.command_mode = true;
        state.command_input = Some(CommandInputState {
            input: "quit".to_string(),
            cursor_pos: 2,
            history_index: 0,
        });
        let widget = CommandBarWidget::new();
        let _para = widget.render(&state);
        // TODO: test content
    }

    #[test]
    fn render_not_in_command_mode() {
        let state = TuiState::default();
        let widget = CommandBarWidget::new();
        let _para = widget.render(&state);
        // TODO: test content
    }
}
