use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use avix_client_core::state::ActiveAgent;

#[derive(Debug, Clone, Default)]
pub struct AgentListWidget {
    pub selected_index: usize,
    #[allow(dead_code)]
    pub scroll_offset: usize,
}

impl AgentListWidget {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn select_next(&mut self, agents: &[ActiveAgent]) {
        if !agents.is_empty() {
            self.selected_index = (self.selected_index + 1).min(agents.len() - 1);
        }
    }

    pub fn select_prev(&mut self, _agents: &[ActiveAgent]) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn render(&self, agents: &[ActiveAgent], _area: Rect) -> List<'_> {
        let items: Vec<ListItem> = agents
            .iter()
            .enumerate()
            .map(|(i, agent)| {
                let status_icon = match agent.status {
                    avix_client_core::atp::types::AgentStatus::Running => "▶",
                    _ => "■",
                };
                let line = format!(
                    "{} {} (PID {}) - {}",
                    status_icon, agent.name, agent.pid, agent.goal
                );
                let mut style = Style::default();
                if i == self.selected_index {
                    style = style.bg(Color::Blue).fg(Color::White);
                }
                ListItem::new(line).style(style)
            })
            .collect();

        List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Agents"))
            .highlight_style(Style::default().bg(Color::Blue).fg(Color::White))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avix_client_core::atp::types::AgentStatus;

    #[test]
    fn render_with_three_agents() {
        let widget = AgentListWidget::new();
        let agents = vec![
            ActiveAgent {
                pid: 1,
                name: "agent1".into(),
                session_id: "s1".into(),
                status: AgentStatus::Running,
                goal: "goal1".into(),
            },
            ActiveAgent {
                pid: 2,
                name: "agent2".into(),
                session_id: "s2".into(),
                status: AgentStatus::Stopped,
                goal: "goal2".into(),
            },
            ActiveAgent {
                pid: 3,
                name: "agent3".into(),
                session_id: "s3".into(),
                status: AgentStatus::Running,
                goal: "goal3".into(),
            },
        ];

        let area = Rect::new(0, 0, 50, 10);
        let _list = widget.render(&agents, area);

        // Just check that render doesn't panic and returns a list
        // Since List internals are private, we can't test content directly
        // In real usage, we'd render it in a frame and snapshot test
        assert!(true);
    }

    #[test]
    fn select_next_wraps() {
        let mut widget = AgentListWidget::new();
        let agents = vec![
            ActiveAgent {
                pid: 1,
                name: "agent1".into(),
                session_id: "s1".into(),
                status: AgentStatus::Running,
                goal: "goal1".into(),
            },
            ActiveAgent {
                pid: 2,
                name: "agent2".into(),
                session_id: "s2".into(),
                status: AgentStatus::Running,
                goal: "goal2".into(),
            },
        ];

        assert_eq!(widget.selected_index, 0);
        widget.select_next(&agents);
        assert_eq!(widget.selected_index, 1);
        widget.select_next(&agents);
        assert_eq!(widget.selected_index, 1); // doesn't go beyond
    }

    #[test]
    fn select_prev_saturates() {
        let mut widget = AgentListWidget {
            selected_index: 1,
            ..Default::default()
        };
        let agents = vec![
            ActiveAgent {
                pid: 1,
                name: "agent1".into(),
                session_id: "s1".into(),
                status: AgentStatus::Running,
                goal: "goal1".into(),
            },
            ActiveAgent {
                pid: 2,
                name: "agent2".into(),
                session_id: "s2".into(),
                status: AgentStatus::Running,
                goal: "goal2".into(),
            },
        ];

        widget.select_prev(&agents);
        assert_eq!(widget.selected_index, 0);
        widget.select_prev(&agents);
        assert_eq!(widget.selected_index, 0); // doesn't go below 0
    }
}
