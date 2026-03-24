use std::io::{stdout, Stdout};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time;

use avix_client_core::atp::event_emitter::EventEmitter;
use avix_client_core::atp::types::{Event as AtpEvent, EventBody, EventKind};
use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::commands::{list_agents, resolve_hil, spawn_agent};
use avix_client_core::config::ClientConfig;
use avix_client_core::notification::{HilState, Notification, NotificationKind, NotificationStore};
use avix_client_core::persistence;
use avix_client_core::server::ServerHandle;
use avix_client_core::state::{new_shared, SharedState};

use super::state::{Action, NewAgentFormState, TuiState};
use super::widgets::hil_modal::render_hil_modal;
use super::widgets::new_agent_form::NewAgentFormWidget;

use crate::Cli;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

type Tui = Terminal<CrosstermBackend<Stdout>>;

async fn dispatch_event(
    state: &SharedState,
    notifications: &NotificationStore,
    event: AtpEvent,
    action_tx: &tokio::sync::mpsc::Sender<Action>,
) {
    match event.kind {
        EventKind::AgentOutput => {
            if let EventBody::AgentOutput(body) = event.body {
                // Send action to update TuiState
                let _ = action_tx
                    .send(Action::UpdateAgentOutput(body.pid, body.text))
                    .await;
            }
        }
        EventKind::AgentStatus => {
            if let EventBody::AgentStatus(body) = event.body {
                // Update agent status
                let s = state.read().await;
                let mut agents = s.agents.write().await;
                if let Some(agent) = agents.iter_mut().find(|a| a.pid == body.pid) {
                    agent.status = body.status;
                }
            }
        }
        EventKind::AgentExit => {
            if let EventBody::AgentExit(body) = event.body {
                // Add notification
                let notif = Notification::from_agent_exit(
                    body.pid,
                    &body.session_id,
                    body.reason.as_deref(),
                );
                notifications.add(notif).await;
                let _ = persistence::save_notifications(&notifications.all().await);
            }
        }
        EventKind::HilRequest => {
            if let EventBody::HilRequest(body) = event.body {
                // Add HIL notification
                let notif = Notification::from_hil_request(&body);
                notifications.add(notif).await;
                let _ = persistence::save_notifications(&notifications.all().await);
                // Send action to set pending_hil
                let hil = HilState {
                    pid: body.pid,
                    hil_id: body.hil_id,
                    approval_token: body.approval_token,
                    prompt: body.prompt,
                    timeout_secs: body.timeout_secs,
                    outcome: None,
                };
                let _ = action_tx.send(Action::SetPendingHil(Some(hil))).await;
            }
        }
        EventKind::HilResolved => {
            if let EventBody::HilResolved(body) = event.body {
                // Resolve in notifications
                notifications.resolve_hil(&body.hil_id, body.outcome).await;
                let _ = persistence::save_notifications(&notifications.all().await);
                // Send action to clear pending_hil
                let _ = action_tx.send(Action::SetPendingHil(None)).await;
            }
        }
        EventKind::SysAlert => {
            if let EventBody::SysAlert(body) = event.body {
                // Add notification
                let notif = Notification::from_sys_alert(&body.level, &body.message);
                notifications.add(notif).await;
                let _ = persistence::save_notifications(&notifications.all().await);
            }
        }
        _ => {} // Ignore other events
    }
}

async fn connect_atp(url: &str, token: &str) -> Result<Dispatcher> {
    let client = AtpClient::connect(url, "user", token).await?;
    let dispatcher = Dispatcher::new(client);
    Ok(dispatcher)
}

async fn update_state_from_shared(state: &mut TuiState, shared: &SharedState) {
    #[derive(Deserialize)]
    struct AgentInfo {
        pid: u64,
        name: String,
        session_id: String,
        status: String,
        goal: String,
    }

    let s = shared.read().await;
    state.connected = matches!(
        s.connection_status,
        avix_client_core::state::ConnectionStatus::Connected { .. }
    );

    if state.connected {
        if let Some(dispatcher) = &s.dispatcher {
            // Fetch agents
            if let Ok(agents) = list_agents(dispatcher, "token").await {
                // Convert to ActiveAgent
                let active_agents: Vec<_> = agents
                    .into_iter()
                    .filter_map(|a| {
                        serde_json::from_value::<AgentInfo>(a).ok().map(|agent| {
                            avix_client_core::state::ActiveAgent {
                                pid: agent.pid,
                                name: agent.name,
                                session_id: agent.session_id,
                                status: match agent.status.as_str() {
                                    "running" => avix_client_core::atp::types::AgentStatus::Running,
                                    _ => avix_client_core::atp::types::AgentStatus::Stopped,
                                },
                                goal: agent.goal,
                            }
                        })
                    })
                    .collect();
                *s.agents.write().await = active_agents;
            }
        }
    }

    state.agents = s.agents.read().await.clone();
    state.notifications = s.notifications.all().await;
    state.notifications_count = state.notifications.iter().filter(|n| !n.read).count();
    // Count HIL pending from notifications
    state.hil_pending = state
        .notifications
        .iter()
        .filter(|n| n.kind == NotificationKind::Hil && n.hil.as_ref().unwrap().outcome.is_none())
        .count();
}

pub async fn run(cli: Cli) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, cli).await;
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Tui> {
    let mut stdout = stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

async fn run_app(terminal: &mut Tui, cli: Cli) -> Result<()> {
    let mut state = TuiState::default();
    let client_config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
    let shared_state = new_shared(client_config.clone());
    let (action_tx, mut action_rx) = mpsc::channel(100);

    // Load persisted notifications
    if let Ok(notifications) = persistence::load_notifications() {
        for notif in notifications {
            shared_state.read().await.notifications.add(notif).await;
        }
    }

    loop {
        // Drain action channel
        while let Ok(action) = action_rx.try_recv() {
            state.reducer(action);
        }

        // Update TuiState from shared_state
        update_state_from_shared(&mut state, &shared_state).await;

        terminal.draw(|f| ui(f, &state))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if let Some(hil) = &state.pending_hil {
                    // Handle HIL modal keys
                    match key.code {
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ = resolve_hil(
                                    dispatcher,
                                    "token",
                                    hil.pid,
                                    &hil.hil_id,
                                    &hil.approval_token,
                                    true,
                                    None,
                                )
                                .await;
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ = resolve_hil(
                                    dispatcher,
                                    "token",
                                    hil.pid,
                                    &hil.hil_id,
                                    &hil.approval_token,
                                    false,
                                    None,
                                )
                                .await;
                            }
                        }
                        KeyCode::Esc => {
                            // Dismiss without responding
                            state.reducer(Action::SetPendingHil(None));
                        }
                        _ => {}
                    }
                } else if let Some(form) = &state.new_agent_form.clone() {
                    // Form input mode
                    match key.code {
                        KeyCode::Tab => {
                            let mut new_form = form.clone();
                            new_form.focused_field = 1 - new_form.focused_field; // toggle 0/1
                            state.reducer(Action::SetNewAgentForm(Some(new_form)));
                        }
                        KeyCode::Enter => {
                            // Submit form
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ =
                                    spawn_agent(dispatcher, "token", &form.name, &form.goal, &[])
                                        .await;
                            }
                            state.reducer(Action::SetNewAgentForm(None));
                        }
                        KeyCode::Esc => {
                            state.reducer(Action::SetNewAgentForm(None));
                        }
                        KeyCode::Char(c) => {
                            let mut new_form = form.clone();
                            if form.focused_field == 0 {
                                new_form.name.push(c);
                            } else {
                                new_form.goal.push(c);
                            }
                            state.reducer(Action::SetNewAgentForm(Some(new_form)));
                        }
                        KeyCode::Backspace => {
                            let mut new_form = form.clone();
                            if form.focused_field == 0 {
                                new_form.name.pop();
                            } else {
                                new_form.goal.pop();
                            }
                            state.reducer(Action::SetNewAgentForm(Some(new_form)));
                        }
                        _ => {}
                    }
                } else if state.notifications_popup_open {
                    // Notifications popup mode
                    match key.code {
                        KeyCode::Esc => {
                            state.reducer(Action::ToggleNotificationsPopup);
                        }
                        KeyCode::Enter => {
                            // Mark selected as read
                            if let Some(notif) = state
                                .notifications
                                .get(state.notification_bar_widget.selected_index)
                            {
                                shared_state
                                    .read()
                                    .await
                                    .notifications
                                    .mark_read(&notif.id)
                                    .await;
                            }
                        }
                        KeyCode::Up => {
                            state
                                .notification_bar_widget
                                .select_prev(&state.notifications);
                        }
                        KeyCode::Down => {
                            state
                                .notification_bar_widget
                                .select_next(&state.notifications);
                        }
                        _ => {}
                    }
                } else {
                    // Normal mode
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') => {
                            if !state.connected {
                                // Ensure server is running
                                let _server_handle =
                                    ServerHandle::ensure_running(&client_config).await?;
                                if let Ok(dispatcher) = connect_atp(&cli.url, &cli.token).await {
                                    let dispatcher = Arc::new(dispatcher);
                                    let dispatcher_c = Arc::clone(&dispatcher);
                                    let emitter = EventEmitter::start(move || {
                                        let d = Arc::clone(&dispatcher_c);
                                        async move { Ok(Arc::try_unwrap(d).unwrap()) }
                                    });
                                    {
                                        let mut s = shared_state.write().await;
                                        s.dispatcher = Some(Arc::clone(&dispatcher));
                                        s.emitter = Some(emitter);
                                        s.connection_status =
                                            avix_client_core::state::ConnectionStatus::Connected {
                                                session_id: "tui-session".into(),
                                            };

                                        // Start background event task
                                        if let Some(emitter) = &s.emitter {
                                            let rx = emitter.subscribe_all();
                                            let state_c = Arc::clone(&shared_state);
                                            let notifications = Arc::clone(&s.notifications);
                                            let action_tx_c = action_tx.clone();
                                            tokio::spawn(async move {
                                                let mut rx = rx;
                                                while let Ok(event) = rx.recv().await {
                                                    dispatch_event(
                                                        &state_c,
                                                        &notifications,
                                                        event,
                                                        &action_tx_c,
                                                    )
                                                    .await;
                                                }
                                            });
                                        }
                                    }
                                    state.reducer(Action::Connect);
                                }
                            }
                        }
                        KeyCode::Char('a') => {
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ = spawn_agent(
                                    dispatcher,
                                    "token",
                                    "test-agent",
                                    "test goal",
                                    &[],
                                )
                                .await;
                            }
                        }
                        KeyCode::Char('f') => {
                            // Toggle new agent form
                            let new_form = if state.new_agent_form.is_some() {
                                None
                            } else {
                                Some(NewAgentFormState {
                                    name: String::new(),
                                    goal: String::new(),
                                    focused_field: 0,
                                })
                            };
                            state.reducer(Action::SetNewAgentForm(new_form));
                        }
                        KeyCode::Char('n') => {
                            state.reducer(Action::ToggleNotificationsPopup);
                        }
                        KeyCode::Up => {
                            state.agent_list_widget.select_prev(&state.agents);
                        }
                        KeyCode::Down => {
                            state.agent_list_widget.select_next(&state.agents);
                        }
                        _ => {}
                    }
                }
            }
        }

        time::sleep(Duration::from_millis(100)).await;
    }
}

fn ui(f: &mut ratatui::Frame, state: &TuiState) {
    // If HIL pending, render modal
    if let (Some(hil), Some(started)) = (&state.pending_hil, state.hil_started_at) {
        render_hil_modal(f, hil, started);
        return;
    }

    // If new agent form open, render it
    if let Some(form) = &state.new_agent_form {
        let widget = NewAgentFormWidget::new();
        let size = f.size();
        let widgets = widget.render(form, size);
        for (area, para) in widgets {
            f.render_widget(para, area);
        }
        return;
    }

    // If notifications popup open, render it
    if state.notifications_popup_open {
        let size = f.size();
        let popup_area = Rect {
            x: size.width / 4,
            y: size.height / 4,
            width: size.width / 2,
            height: size.height / 2,
        };
        let list = state
            .notification_bar_widget
            .render_popup(&state.notifications, popup_area);
        f.render_widget(list, popup_area);
        return;
    }

    let size = f.size();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // status bar
            Constraint::Length(10), // agents list
            Constraint::Min(1),     // output pane
            Constraint::Length(1),  // notification bar
        ])
        .split(size);

    // Status bar
    let status = if state.connected {
        "Connected"
    } else {
        "Disconnected"
    };
    let status_bar =
        Paragraph::new(status).style(Style::default().fg(Color::White).bg(Color::Blue));
    f.render_widget(status_bar, layout[0]);

    // Agents list
    let agents_list = state.agent_list_widget.render(&state.agents, layout[1]);
    f.render_widget(agents_list, layout[1]);

    // Agent output pane
    if let Some(selected_pid) = state
        .agents
        .get(state.agent_list_widget.selected_index)
        .map(|a| a.pid)
    {
        if let Some(buf) = state.agent_output_buffers.get(&selected_pid) {
            let output_para = buf.render(selected_pid, layout[2]);
            f.render_widget(output_para, layout[2]);
        } else {
            let empty = Paragraph::new("No output yet").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Agent {} Output", selected_pid)),
            );
            f.render_widget(empty, layout[2]);
        }
    } else {
        let no_selection = Paragraph::new("No agent selected")
            .block(Block::default().borders(Borders::ALL).title("Agent Output"));
        f.render_widget(no_selection, layout[2]);
    }

    // Notifications bar
    let notifs_bar = state
        .notification_bar_widget
        .render_bar(state.notifications_count);
    f.render_widget(notifs_bar, layout[3]);
}

impl TuiState {
    // Add reducer logic if needed
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_key_handling_stub() {
        // Stub test for key handling
    }
}
