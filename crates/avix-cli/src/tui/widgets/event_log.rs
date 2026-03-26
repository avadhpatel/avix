use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem};

use crate::tui::state::{EventLog, TuiEvent};
use avix_client_core::atp::types::EventKind;

/// EventLogWidget renders the event log pane with recent events.
#[derive(Debug, Clone, Default)]
pub struct EventLogWidget;

#[allow(dead_code)]
impl EventLogWidget {
    pub fn new() -> Self {
        Self
    }

    /// Render the event log.
    /// Returns a List widget.
    pub fn render(&self, log: &EventLog, area: Rect) -> List<'_> {
        let items: Vec<ListItem> = log
            .events()
            .iter()
            .rev() // Most recent first
            .take(10)
            .map(|event| {
                let (icon, summary) = match event {
                    TuiEvent::SentCommand { cmd, .. } => ("➤", format!("Sent: {}", cmd)),
                    TuiEvent::ReceivedAtp {
                        kind, pid, summary, ..
                    } => {
                        let icon = match kind {
                            EventKind::AgentOutput => "📥",
                            EventKind::AgentStatus => "🔄",
                            EventKind::AgentExit => "❌",
                            EventKind::HilRequest => "⚠",
                            EventKind::HilResolved => "✅",
                            EventKind::SysAlert => "🔔",
                            _ => "📄",
                        };
                        let pid_str = pid.map(|p| format!(" pid={}", p)).unwrap_or_default();
                        (icon, format!("{}{}", summary, pid_str))
                    }
                };

                // Truncate summary to fit width
                let max_summary_len = area.width.saturating_sub(15) as usize; // icon + space + timestamp
                let truncated_summary = if summary.len() > max_summary_len {
                    format!("{}...", &summary[..max_summary_len.saturating_sub(3)])
                } else {
                    summary
                };

                let line = format!("{} {}", icon, truncated_summary);
                ListItem::new(line)
            })
            .collect();

        List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Event Log"))
            .highlight_style(Style::default().fg(Color::Yellow))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::{EventLog, TuiEvent};
    use avix_client_core::atp::types::EventKind;
    use std::time::Instant;

    #[test]
    fn render_empty_log() {
        let log = EventLog::default();
        let widget = EventLogWidget::new();
        let list = widget.render(&log, Rect::new(0, 0, 80, 24));
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn render_with_events() {
        let mut log = EventLog::default();
        log.push(TuiEvent::SentCommand {
            cmd: "spawn foo bar".to_string(),
            timestamp: Instant::now(),
        });
        log.push(TuiEvent::ReceivedAtp {
            kind: EventKind::AgentOutput,
            pid: Some(42),
            summary: "Hello world".to_string(),
            timestamp: Instant::now(),
        });
        let widget = EventLogWidget::new();
        let list = widget.render(&log, Rect::new(0, 0, 80, 24));
        assert_eq!(list.len(), 2);
        // TODO: check content when List exposes items
    }

    #[test]
    fn render_truncates_long_summary() {
        let mut log = EventLog::default();
        let long_summary = "A".repeat(100);
        log.push(TuiEvent::ReceivedAtp {
            kind: EventKind::AgentOutput,
            pid: None,
            summary: long_summary,
            timestamp: Instant::now(),
        });
        let widget = EventLogWidget::new();
        let list = widget.render(&log, Rect::new(0, 0, 20, 24)); // Small width
                                                                 // TODO: check truncation
    }
}
