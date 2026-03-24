use std::collections::HashMap;
use std::time::Instant;

use avix_client_core::notification::{HilState, Notification};
use avix_client_core::state::ActiveAgent;

use crate::tui::widgets::agent_list::AgentListWidget;
use crate::tui::widgets::agent_output::AgentOutputBuffer;
use crate::tui::widgets::notification_bar::NotificationBarWidget;

#[derive(Debug, Clone, Default)]
pub struct TuiState {
    pub connected: bool,
    pub agents: Vec<ActiveAgent>,
    pub notifications_count: usize,
    pub hil_pending: usize,
    pub agent_output_buffers: HashMap<u64, AgentOutputBuffer>,
    pub pending_hil: Option<HilState>,
    pub hil_started_at: Option<Instant>,
    pub notifications_popup_open: bool,
    pub new_agent_form: Option<NewAgentFormState>,
    pub notifications: Vec<Notification>,
    pub agent_list_widget: AgentListWidget,
    pub notification_bar_widget: NotificationBarWidget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentFormState {
    pub name: String,
    pub goal: String,
    pub focused_field: usize, // 0 = name, 1 = goal
}

#[allow(dead_code)]
pub enum Action {
    Connect,
    Disconnect,
    UpdateAgents(Vec<ActiveAgent>),
    UpdateNotifications(usize),
    UpdateHilPending(usize),
    UpdateAgentOutput(u64, String), // pid, text
    SetPendingHil(Option<HilState>),
    ToggleNotificationsPopup,
    SetNewAgentForm(Option<NewAgentFormState>),
}

impl TuiState {
    pub fn reducer(&mut self, action: Action) {
        match action {
            Action::Connect => self.connected = true,
            Action::Disconnect => {
                self.connected = false;
                self.agents.clear();
                self.hil_pending = 0;
                self.agent_output_buffers.clear();
                self.pending_hil = None;
                self.hil_started_at = None;
                self.notifications_popup_open = false;
                self.new_agent_form = None;
            }
            Action::UpdateAgents(agents) => self.agents = agents,
            Action::UpdateNotifications(count) => self.notifications_count = count,
            Action::UpdateHilPending(count) => self.hil_pending = count,
            Action::UpdateAgentOutput(pid, text) => {
                self.agent_output_buffers
                    .entry(pid)
                    .or_default()
                    .push_text(&text);
            }
            Action::SetPendingHil(hil) => {
                let started = hil.as_ref().map(|_| Instant::now());
                self.pending_hil = hil;
                self.hil_started_at = started;
            }
            Action::ToggleNotificationsPopup => {
                self.notifications_popup_open = !self.notifications_popup_open
            }
            Action::SetNewAgentForm(form) => self.new_agent_form = form,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avix_client_core::atp::types::AgentStatus;

    #[test]
    fn default_state_is_disconnected() {
        let state = TuiState::default();
        assert!(!state.connected);
        assert!(state.agents.is_empty());
        assert_eq!(state.notifications_count, 0);
        assert_eq!(state.hil_pending, 0);
        assert!(state.agent_output_buffers.is_empty());
        assert!(state.pending_hil.is_none());
        assert!(state.hil_started_at.is_none());
        assert!(!state.notifications_popup_open);
        assert!(state.new_agent_form.is_none());
        assert!(state.notifications.is_empty());
        assert_eq!(state.agent_list_widget.selected_index, 0);
        assert_eq!(state.notification_bar_widget.selected_index, 0);
    }

    #[test]
    fn connect_action_sets_connected() {
        let mut state = TuiState::default();
        state.reducer(Action::Connect);
        assert!(state.connected);
    }

    #[test]
    fn disconnect_action_clears_state() {
        let mut state = TuiState {
            connected: true,
            agents: vec![ActiveAgent {
                pid: 1,
                name: "test".into(),
                session_id: "sid".into(),
                status: AgentStatus::Running,
                goal: "goal".into(),
            }],
            notifications_count: 5,
            hil_pending: 2,
            agent_output_buffers: HashMap::new(),
            pending_hil: Some(HilState {
                hil_id: "test".into(),
                pid: 1,
                approval_token: "token".into(),
                prompt: "prompt".into(),
                timeout_secs: 30,
                outcome: None,
            }),
            hil_started_at: None,
            notifications_popup_open: true,
            new_agent_form: Some(NewAgentFormState {
                name: "name".into(),
                goal: "goal".into(),
                focused_field: 0,
            }),
            notifications: vec![],
            agent_list_widget: AgentListWidget::default(),
            notification_bar_widget: NotificationBarWidget::default(),
        };
        state.reducer(Action::Disconnect);
        assert!(!state.connected);
        assert!(state.agents.is_empty());
        assert_eq!(state.hil_pending, 0);
        assert_eq!(state.notifications_count, 5); // notifications persist on disconnect?
        assert!(state.agent_output_buffers.is_empty());
        assert!(state.pending_hil.is_none());
        assert!(state.hil_started_at.is_none());
        assert!(!state.notifications_popup_open);
        assert!(state.new_agent_form.is_none());
        assert!(state.notifications.is_empty()); // notifications cleared on disconnect?
                                                 // widgets reset to default
        assert_eq!(state.agent_list_widget.selected_index, 0);
        assert_eq!(state.notification_bar_widget.selected_index, 0);
    }

    #[test]
    fn update_agents_sets_agents() {
        let mut state = TuiState::default();
        let agents = vec![ActiveAgent {
            pid: 1,
            name: "agent1".into(),
            session_id: "sid".into(),
            status: AgentStatus::Running,
            goal: "goal".into(),
        }];
        state.reducer(Action::UpdateAgents(agents.clone()));
        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.agents[0].pid, 1);
        assert_eq!(state.agents[0].name, "agent1");
    }

    #[test]
    fn update_notifications_sets_count() {
        let mut state = TuiState::default();
        state.reducer(Action::UpdateNotifications(10));
        assert_eq!(state.notifications_count, 10);
    }

    #[test]
    fn update_hil_pending_sets_count() {
        let mut state = TuiState::default();
        state.reducer(Action::UpdateHilPending(3));
        assert_eq!(state.hil_pending, 3);
    }

    #[test]
    fn update_agent_output_adds_to_buffer() {
        let mut state = TuiState::default();
        state.reducer(Action::UpdateAgentOutput(42, "hello\nworld\n".into()));
        let buf = state.agent_output_buffers.get(&42).unwrap();
        assert_eq!(buf.lines_len(), 2);
        assert_eq!(buf.get_line(0), Some("hello"));
        assert_eq!(buf.get_line(1), Some("world"));
    }

    #[test]
    fn set_pending_hil_sets_hil() {
        let mut state = TuiState::default();
        let hil = HilState {
            hil_id: "test".into(),
            pid: 1,
            approval_token: "token".into(),
            prompt: "prompt".into(),
            timeout_secs: 30,
            outcome: None,
        };
        state.reducer(Action::SetPendingHil(Some(hil.clone())));
        assert_eq!(state.pending_hil, Some(hil));
    }

    #[test]
    fn toggle_notifications_popup_toggles() {
        let mut state = TuiState::default();
        assert!(!state.notifications_popup_open);
        state.reducer(Action::ToggleNotificationsPopup);
        assert!(state.notifications_popup_open);
        state.reducer(Action::ToggleNotificationsPopup);
        assert!(!state.notifications_popup_open);
    }

    #[test]
    fn set_new_agent_form_sets_form() {
        let mut state = TuiState::default();
        let form = NewAgentFormState {
            name: "agent".into(),
            goal: "goal".into(),
            focused_field: 1,
        };
        state.reducer(Action::SetNewAgentForm(Some(form.clone())));
        assert_eq!(state.new_agent_form, Some(form));
    }
}
