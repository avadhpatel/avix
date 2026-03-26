use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::state::TuiState;

/// StatusWidget renders the top status bar with connection, agent counts, notifications, etc.
#[derive(Debug, Clone, Default)]
pub struct StatusWidget;

#[allow(dead_code)]
impl StatusWidget {
    pub fn new() -> Self {
        Self
    }

    /// Render the status bar.
    /// Returns a Paragraph widget.
    pub fn render(&self, state: &TuiState) -> Paragraph<'_> {
        let connection_status = if state.connected {
            "Connected"
        } else {
            "Disconnected"
        };

        let total_agents = state.agents.len();
        let running_agents = state
            .agents
            .iter()
            .filter(|a| matches!(a.status, avix_client_core::atp::types::AgentStatus::Running))
            .count();

        let agents_status = format!("Agents: {}/{}", running_agents, total_agents);
        let notifs_status = format!("Notifs: {}", state.notifications_count);
        let hil_status = format!("HIL: {}", state.hil_pending);

        // For uptime, we could track start time, but for now, placeholder
        let uptime_status = "Uptime: --:--:--"; // TODO: implement uptime tracking

        let status_text = format!(
            "{} | {} | {} | {} | {}",
            connection_status, agents_status, notifs_status, hil_status, uptime_status
        );

        Paragraph::new(status_text)
            .style(Style::default().fg(Color::White).bg(Color::Blue))
            .block(Block::default().borders(Borders::BOTTOM))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::TuiState;
    use avix_client_core::atp::types::AgentStatus;
    use avix_client_core::state::ActiveAgent;

    #[test]
    fn render_disconnected() {
        let state = TuiState::default();
        let widget = StatusWidget::new();
        let _para = widget.render(&state);
        // TODO: test content when Paragraph exposes text
    }

    #[test]
    fn render_connected_with_agents() {
        let state = TuiState {
            connected: true,
            agents: vec![
                ActiveAgent {
                    pid: 1,
                    name: "agent1".into(),
                    session_id: "sid".into(),
                    status: AgentStatus::Running,
                    goal: "goal".into(),
                },
                ActiveAgent {
                    pid: 2,
                    name: "agent2".into(),
                    session_id: "sid".into(),
                    status: AgentStatus::Stopped,
                    goal: "goal".into(),
                },
            ],
            notifications_count: 5,
            hil_pending: 2,
            ..Default::default()
        };
        let widget = StatusWidget::new();
        let _para = widget.render(&state);
        // TODO: test content when Paragraph exposes text
    }
}
