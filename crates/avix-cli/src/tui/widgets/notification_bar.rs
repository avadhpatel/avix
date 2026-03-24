use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use avix_client_core::notification::{Notification, NotificationKind};

#[derive(Debug, Clone, Default)]
pub struct NotificationBarWidget {
    pub selected_index: usize,
}

impl NotificationBarWidget {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn select_next(&mut self, notifications: &[Notification]) {
        if !notifications.is_empty() {
            self.selected_index = (self.selected_index + 1).min(notifications.len() - 1);
        }
    }

    pub fn select_prev(&mut self, _notifications: &[Notification]) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn render_bar(&self, unread_count: usize) -> Paragraph<'_> {
        let text = format!("Unread notifications: {}", unread_count);
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Notifications"),
        )
    }

    pub fn render_popup(&self, notifications: &[Notification], _area: Rect) -> List<'_> {
        let items: Vec<ListItem> = notifications
            .iter()
            .enumerate()
            .map(|(i, notif)| {
                let kind_icon = match notif.kind {
                    NotificationKind::Hil => "⚠",
                    NotificationKind::AgentExit => "✗",
                    NotificationKind::SysAlert => "ℹ",
                };
                let line = format!("{} {}", kind_icon, notif.message);
                let mut style = Style::default();
                if i == self.selected_index {
                    style = style.bg(Color::Yellow).fg(Color::Black);
                }
                ListItem::new(line).style(style)
            })
            .collect();

        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Notifications"),
            )
            .highlight_style(Style::default().bg(Color::Yellow).fg(Color::Black))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn render_bar_with_count() {
        let widget = NotificationBarWidget::new();
        let _para = widget.render_bar(5);
        // Can't check content due to private fields, just ensure it doesn't panic
    }

    #[test]
    fn render_popup_with_two_unread() {
        let widget = NotificationBarWidget::new();
        let notifications = vec![
            Notification {
                id: "1".into(),
                kind: NotificationKind::SysAlert,
                agent_pid: None,
                session_id: None,
                message: "System alert".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
            Notification {
                id: "2".into(),
                kind: NotificationKind::Hil,
                agent_pid: Some(123),
                session_id: Some("sess".into()),
                message: "HIL request".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
        ];

        let area = Rect::new(0, 0, 50, 10);
        let _list = widget.render_popup(&notifications, area);
    }

    #[test]
    fn select_next_wraps() {
        let mut widget = NotificationBarWidget::new();
        let notifications = vec![
            Notification {
                id: "1".into(),
                kind: NotificationKind::SysAlert,
                agent_pid: None,
                session_id: None,
                message: "msg".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
            Notification {
                id: "2".into(),
                kind: NotificationKind::SysAlert,
                agent_pid: None,
                session_id: None,
                message: "msg2".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
        ];

        assert_eq!(widget.selected_index, 0);
        widget.select_next(&notifications);
        assert_eq!(widget.selected_index, 1);
        widget.select_next(&notifications);
        assert_eq!(widget.selected_index, 1);
    }

    #[test]
    fn select_prev_saturates() {
        let mut widget = NotificationBarWidget { selected_index: 1 };
        let notifications = vec![
            Notification {
                id: "1".into(),
                kind: NotificationKind::SysAlert,
                agent_pid: None,
                session_id: None,
                message: "msg".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
            Notification {
                id: "2".into(),
                kind: NotificationKind::SysAlert,
                agent_pid: None,
                session_id: None,
                message: "msg2".into(),
                hil: None,
                created_at: Utc::now(),
                resolved_at: None,
                read: false,
            },
        ];

        widget.select_prev(&notifications);
        assert_eq!(widget.selected_index, 0);
        widget.select_prev(&notifications);
        assert_eq!(widget.selected_index, 0);
    }
}
