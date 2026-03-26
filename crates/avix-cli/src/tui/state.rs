use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use avix_client_core::atp::types::EventKind;
use tracing::debug;
use avix_client_core::notification::{HilState, Notification};
use avix_client_core::state::ActiveAgent;

use crate::tui::widgets::agent_list::AgentListWidget;
use crate::tui::widgets::agent_output::AgentOutputBuffer;
use crate::tui::widgets::command_bar::CommandBarWidget;
use crate::tui::widgets::event_log::EventLogWidget;
use crate::tui::widgets::help_modal::HelpModalWidget;
use crate::tui::widgets::notification_bar::NotificationBarWidget;
use crate::tui::widgets::status::StatusWidget;

#[derive(Debug, Clone, Default)]
pub struct CommandInputState {
    pub input: String,
    pub cursor_pos: usize,
    pub history_index: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum TuiEvent {
    SentCommand {
        cmd: String,
        #[allow(dead_code)]
        timestamp: Instant,
    },
    ReceivedAtp {
        kind: EventKind,
        pid: Option<u64>,
        summary: String,
        #[allow(dead_code)]
        timestamp: Instant,
    },
}

#[derive(Debug, Clone, Default)]
pub struct EventLog {
    events: VecDeque<TuiEvent>,
}

impl EventLog {
    pub fn push(&mut self, event: TuiEvent) {
        self.events.push_back(event);
        if self.events.len() > 10 {
            self.events.pop_front();
        }
    }

    pub fn events(&self) -> &VecDeque<TuiEvent> {
        &self.events
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum ParsedCommand {
    Quit,
    Connect,
    Spawn {
        name: String,
        goal: String,
    },
    Kill {
        pid: u64,
    },
    Help,
    ToggleLogs,
    ToggleNotifications,
    ToggleNewAgentForm,
    #[allow(dead_code)]
    Invalid(String),
}

#[derive(Debug, Clone)]
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
    // New fields for command input and logging
    pub command_mode: bool,
    pub command_input: Option<CommandInputState>,
    pub command_history: Vec<String>,
    pub event_log: EventLog,
    pub log_visible: bool,
    pub help_modal_open: bool,
    // New widget instances
    pub status_widget: StatusWidget,
    pub command_bar_widget: CommandBarWidget,
    pub event_log_widget: EventLogWidget,
    pub help_modal_widget: HelpModalWidget,
    pub startup_time: Instant,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            connected: false,
            agents: Vec::new(),
            notifications_count: 0,
            hil_pending: 0,
            agent_output_buffers: HashMap::new(),
            pending_hil: None,
            hil_started_at: None,
            notifications_popup_open: false,
            new_agent_form: None,
            notifications: Vec::new(),
            agent_list_widget: AgentListWidget::default(),
            notification_bar_widget: NotificationBarWidget::default(),
            command_mode: false,
            command_input: None,
            command_history: Vec::new(),
            event_log: EventLog::default(),
            log_visible: false,
            help_modal_open: false,
            status_widget: StatusWidget,
            command_bar_widget: CommandBarWidget,
            event_log_widget: EventLogWidget,
            help_modal_widget: HelpModalWidget,
            startup_time: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewAgentFormState {
    pub name: String,
    pub goal: String,
    pub focused_field: usize, // 0 = name, 1 = goal
}

#[derive(Debug, Clone)]
pub enum InputDelta {
    Char(char),
    Backspace,
    Left,
    Right,
    HistoryUp,
    HistoryDown,
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
    // New actions for command input and logging
    EnterCommandMode,
    ExitCommandMode,
    UpdateCommandInput(InputDelta),
    SubmitCommand(String),
    LogEvent(TuiEvent),
    ToggleLogs,
    ToggleHelpModal,
    /// Close the help modal. Used when pressing Esc in help modal mode.
    /// See [TUI Key Bindings Reference](docs/architecture/tui.md#key-bindings).
    CloseHelpModal,
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
                self.command_mode = false;
                self.command_input = None;
                self.command_history.clear();
                self.event_log = EventLog::default();
                self.log_visible = false;
                self.help_modal_open = false;
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
            Action::EnterCommandMode => {
                self.command_mode = true;
                self.command_input = Some(CommandInputState {
                    input: String::new(),
                    cursor_pos: 0,
                    history_index: self.command_history.len(),
                });
            }
            Action::ExitCommandMode => {
                self.command_mode = false;
                self.command_input = None;
            }
            Action::UpdateCommandInput(delta) => {
                if let Some(input_state) = &mut self.command_input {
                    match delta {
                        InputDelta::Char(c) => {
                            input_state.input.insert(input_state.cursor_pos, c);
                            input_state.cursor_pos += 1;
                        }
                        InputDelta::Backspace => {
                            if input_state.cursor_pos > 0 {
                                input_state.cursor_pos -= 1;
                                input_state.input.remove(input_state.cursor_pos);
                            }
                        }
                        InputDelta::Left => {
                            if input_state.cursor_pos > 0 {
                                input_state.cursor_pos -= 1;
                            }
                        }
                        InputDelta::Right => {
                            if input_state.cursor_pos < input_state.input.len() {
                                input_state.cursor_pos += 1;
                            }
                        }
                        InputDelta::HistoryUp => {
                            if input_state.history_index > 0 {
                                input_state.history_index -= 1;
                                input_state.input =
                                    self.command_history[input_state.history_index].clone();
                                input_state.cursor_pos = input_state.input.len();
                            }
                        }
                        InputDelta::HistoryDown => {
                            if input_state.history_index < self.command_history.len() {
                                input_state.history_index += 1;
                                if input_state.history_index == self.command_history.len() {
                                    input_state.input.clear();
                                } else {
                                    input_state.input =
                                        self.command_history[input_state.history_index].clone();
                                }
                                input_state.cursor_pos = input_state.input.len();
                            }
                        }
                    }
                }
            }
            Action::SubmitCommand(cmd) => {
                self.command_history.push(cmd);
                self.command_mode = false;
                self.command_input = None;
            }
            Action::LogEvent(event) => {
                self.event_log.push(event);
            }
            Action::ToggleLogs => {
                self.log_visible = !self.log_visible;
            }
            Action::ToggleHelpModal => {
                self.help_modal_open = !self.help_modal_open;
            }
            Action::CloseHelpModal => {
                debug!("Closing help modal");
                self.help_modal_open = false;
            }
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
        assert!(!state.command_mode);
        assert!(state.command_input.is_none());
        assert!(state.command_history.is_empty());
        assert_eq!(state.event_log.events().len(), 0);
        assert!(!state.log_visible);
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
            command_mode: true,
            command_input: Some(CommandInputState {
                input: "test".into(),
                cursor_pos: 4,
                history_index: 1,
            }),
            command_history: vec!["prev".into()],
            event_log: EventLog::default(),
            log_visible: true,
            help_modal_open: true,
            status_widget: StatusWidget,
            command_bar_widget: CommandBarWidget,
            event_log_widget: EventLogWidget,
            help_modal_widget: HelpModalWidget,
            startup_time: Instant::now(),
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
        assert!(!state.command_mode);
        assert!(state.command_input.is_none());
        assert!(state.command_history.is_empty());
        assert_eq!(state.event_log.events().len(), 0);
        assert!(!state.log_visible);
        assert!(!state.help_modal_open);
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

    #[test]
    fn enter_command_mode_sets_mode_and_input() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        assert!(state.command_mode);
        assert!(state.command_input.is_some());
        let input = state.command_input.as_ref().unwrap();
        assert_eq!(input.input, "");
        assert_eq!(input.cursor_pos, 0);
        assert_eq!(input.history_index, 0);
    }

    #[test]
    fn exit_command_mode_clears_mode_and_input() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        assert!(state.command_mode);
        state.reducer(Action::ExitCommandMode);
        assert!(!state.command_mode);
        assert!(state.command_input.is_none());
    }

    #[test]
    fn update_command_input_char_inserts_at_cursor() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('h')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('i')));
        let input = state.command_input.as_ref().unwrap();
        assert_eq!(input.input, "hi");
        assert_eq!(input.cursor_pos, 2);
    }

    #[test]
    fn update_command_input_backspace_removes_at_cursor() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('h')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('i')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Left));
        state.reducer(Action::UpdateCommandInput(InputDelta::Backspace));
        let input = state.command_input.as_ref().unwrap();
        assert_eq!(input.input, "i");
        assert_eq!(input.cursor_pos, 0);
    }

    #[test]
    fn update_command_input_left_right_moves_cursor() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('a')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('b')));
        assert_eq!(state.command_input.as_ref().unwrap().cursor_pos, 2);
        state.reducer(Action::UpdateCommandInput(InputDelta::Left));
        assert_eq!(state.command_input.as_ref().unwrap().cursor_pos, 1);
        state.reducer(Action::UpdateCommandInput(InputDelta::Right));
        assert_eq!(state.command_input.as_ref().unwrap().cursor_pos, 2);
    }

    #[test]
    fn submit_command_adds_to_history_and_exits_mode() {
        let mut state = TuiState::default();
        state.reducer(Action::EnterCommandMode);
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('t')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('e')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('s')));
        state.reducer(Action::UpdateCommandInput(InputDelta::Char('t')));
        state.reducer(Action::SubmitCommand("test".into()));
        assert!(!state.command_mode);
        assert!(state.command_input.is_none());
        assert_eq!(state.command_history, vec!["test"]);
        // History should be available when entering command mode again
        state.reducer(Action::EnterCommandMode);
        let input = state.command_input.as_ref().unwrap();
        assert_eq!(input.history_index, 1);
    }

    #[test]
    fn log_event_adds_to_log_and_caps_at_10() {
        let mut state = TuiState::default();
        for i in 0..12 {
            let event = TuiEvent::SentCommand {
                cmd: format!("cmd{}", i),
                timestamp: Instant::now(),
            };
            state.reducer(Action::LogEvent(event));
        }
        assert_eq!(state.event_log.events().len(), 10);
        if let TuiEvent::SentCommand { cmd, .. } = &state.event_log.events()[0] {
            assert_eq!(cmd, "cmd2");
        } else {
            panic!("Expected SentCommand");
        }
    }

    #[test]
    fn toggle_logs_toggles_visibility() {
        let mut state = TuiState::default();
        assert!(!state.log_visible);
        state.reducer(Action::ToggleLogs);
        assert!(state.log_visible);
        state.reducer(Action::ToggleLogs);
        assert!(!state.log_visible);
    }

    #[test]
    fn close_help_modal_sets_false() {
        let mut state = TuiState::default();
        state.reducer(Action::ToggleHelpModal);
        assert!(state.help_modal_open);
        state.reducer(Action::CloseHelpModal);
        assert!(!state.help_modal_open);
    }
}
