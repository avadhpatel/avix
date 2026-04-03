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
use tokio::sync::mpsc;
use tokio::time;
use tracing::{debug, info, warn};

use avix_client_core::atp::types::{Event as AtpEvent, EventBody, EventKind};
use avix_client_core::atp::{AtpClient, Dispatcher};
use avix_client_core::commands::spawn_agent::spawn_agent;
use avix_client_core::commands::{kill_agent, list_agents, resolve_hil};
use avix_client_core::config::ClientConfig;
use avix_client_core::notification::{HilState, Notification, NotificationKind, NotificationStore};
use avix_client_core::persistence;
use avix_client_core::server::ServerHandle;
use avix_client_core::state::{new_shared, SharedState};

use super::state::{Action, InputDelta, NewAgentFormState, TuiEvent, TuiState};
use super::widgets::hil_modal::render_hil_modal;
use super::widgets::new_agent_form::NewAgentFormWidget;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

type Tui = Terminal<CrosstermBackend<Stdout>>;

async fn dispatch_event(
    state: &SharedState,
    notifications: &NotificationStore,
    event: AtpEvent,
    action_tx: &tokio::sync::mpsc::Sender<Action>,
) {
    debug!("Dispatching event: {:?}", event.kind);

    // Log the received event
    let summary = match &event.body {
        EventBody::AgentOutputChunk(body) => {
            format!("AgentOutputChunk pid={} seq={}", body.pid, body.seq)
        }
        EventBody::AgentOutput(body) => format!("AgentOutput pid={}", body.pid),
        EventBody::AgentStatus(body) => {
            format!("AgentStatus pid={} status={:?}", body.pid, body.status)
        }
        EventBody::AgentExit(body) => format!("AgentExit pid={}", body.pid),
        EventBody::HilRequest(body) => format!("HilRequest pid={}", body.pid),
        EventBody::HilResolved(body) => format!("HilResolved hil_id={}", body.hil_id),
        EventBody::SysAlert(body) => format!("SysAlert: {}", body.message),
        _ => format!("{:?}", event.kind),
    };
    let log_event = TuiEvent::ReceivedAtp {
        kind: event.kind.clone(),
        pid: match &event.body {
            EventBody::AgentOutputChunk(body) => Some(body.pid),
            EventBody::AgentOutput(body) => Some(body.pid),
            EventBody::AgentStatus(body) => Some(body.pid),
            EventBody::AgentExit(body) => Some(body.pid),
            EventBody::HilRequest(body) => Some(body.pid),
            _ => None,
        },
        summary,
        timestamp: std::time::Instant::now(),
    };
    let _ = action_tx.send(Action::LogEvent(log_event)).await;

    match event.kind {
        EventKind::AgentOutputChunk => {
            if let EventBody::AgentOutputChunk(body) = event.body {
                // Only forward non-final deltas — final chunk is an empty sentinel.
                if !body.is_final && !body.text_delta.is_empty() {
                    debug!("Agent output chunk: pid={} seq={}", body.pid, body.seq);
                    let _ = action_tx
                        .send(Action::UpdateAgentOutput(body.pid, body.text_delta))
                        .await;
                }
            }
        }
        EventKind::AgentOutput => {
            if let EventBody::AgentOutput(body) = event.body {
                debug!("Agent output: pid={}", body.pid);
                // Send action to update TuiState
                let _ = action_tx
                    .send(Action::UpdateAgentOutput(body.pid, body.text))
                    .await;
            }
        }
        EventKind::AgentStatus => {
            if let EventBody::AgentStatus(body) = event.body {
                debug!("Agent status: pid={}, status={:?}", body.pid, body.status);
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
                debug!("Agent exited: pid={}", body.pid);
                let session_id = event.owner_session.as_deref().unwrap_or("");
                let notif = Notification::from_agent_exit(body.pid, session_id, None);
                notifications.add(notif).await;
                let _ = persistence::save_notifications(&notifications.all().await);
            }
        }
        EventKind::HilRequest => {
            if let EventBody::HilRequest(body) = event.body {
                debug!("HIL request received: pid={}", body.pid);
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
                debug!("HIL resolved: hil_id={}", body.hil_id);
                // Resolve in notifications
                notifications.resolve_hil(&body.hil_id, body.outcome).await;
                let _ = persistence::save_notifications(&notifications.all().await);
                // Send action to clear pending_hil
                let _ = action_tx.send(Action::SetPendingHil(None)).await;
            }
        }
        EventKind::SysAlert => {
            if let EventBody::SysAlert(body) = event.body {
                debug!("Sys alert: {}", body.message);
                // Add notification
                let notif = Notification::from_sys_alert(&body.level, &body.message);
                notifications.add(notif).await;
                let _ = persistence::save_notifications(&notifications.all().await);
            }
        }
        _ => {} // Ignore other events
    }
}

async fn dispatch_parsed_command(
    cmd: super::state::ParsedCommand,
    shared_state: &SharedState,
    client_config: &ClientConfig,
    action_tx: &tokio::sync::mpsc::Sender<Action>,
    current_tui_state: &TuiState,
) -> Result<(), Box<dyn std::error::Error>> {
    use super::state::ParsedCommand;
    match cmd {
        ParsedCommand::Quit => {
            // Quit is handled at the app level, but we can send a notification
            let notif = Notification::from_sys_alert("info", "Quit command received");
            let notifications = &shared_state.read().await.notifications;
            notifications.add(notif).await;
        }
        ParsedCommand::Connect => {
            let log_event = TuiEvent::SentCommand {
                cmd: "connect".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            if !matches!(
                shared_state.read().await.connection_status,
                avix_client_core::state::ConnectionStatus::Connected { .. }
            ) {
                if let Err(e) = ServerHandle::ensure_running(client_config).await {
                    let notif = Notification::from_sys_alert(
                        "error",
                        &format!("Local server failed to start: {}", e),
                    );
                    let notifications = &shared_state.read().await.notifications;
                    notifications.add(notif).await;
                } else {
                    match AtpClient::connect(client_config.clone()).await {
                        Ok(client) => {
                            let dispatcher = Arc::new(Dispatcher::new(client));
                            {
                                let mut s = shared_state.write().await;
                                s.connection_status =
                                    avix_client_core::state::ConnectionStatus::Connected {
                                        session_id: "tui-session".into(),
                                    };
                                let mut rx = dispatcher.events();
                                let state_c = Arc::clone(shared_state);
                                let notifications = Arc::clone(&s.notifications);
                                let action_tx_c = action_tx.clone();
                                tokio::spawn(async move {
                                    loop {
                                        match rx.recv().await {
                                            Ok(event) => {
                                                dispatch_event(
                                                    &state_c,
                                                    &notifications,
                                                    event,
                                                    &action_tx_c,
                                                )
                                                .await;
                                            }
                                            Err(
                                                tokio::sync::broadcast::error::RecvError::Lagged(n),
                                            ) => {
                                                warn!("Event receiver lagged by {} messages", n);
                                            }
                                            Err(_) => {
                                                warn!("ATP event stream closed");
                                                break;
                                            }
                                        }
                                    }
                                });
                                s.dispatcher = Some(dispatcher);
                            }
                            action_tx.send(Action::Connect).await?;
                        }
                        Err(e) => {
                            let notif = Notification::from_sys_alert(
                                "error",
                                &format!("Connection failed: {}", e),
                            );
                            let notifications = &shared_state.read().await.notifications;
                            notifications.add(notif).await;
                        }
                    }
                }
            }
        }
        ParsedCommand::Spawn { name, goal } => {
            let cmd_str = format!("spawn {} \"{}\"", name, goal);
            let log_event = TuiEvent::SentCommand {
                cmd: cmd_str,
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                let _ = spawn_agent(dispatcher, &name, &goal, &[]).await;
            }
        }
        ParsedCommand::Kill { pid } => {
            let cmd_str = format!("kill {}", pid);
            let log_event = TuiEvent::SentCommand {
                cmd: cmd_str,
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                if let Err(e) = kill_agent(dispatcher, pid).await {
                    let notif = Notification::from_sys_alert(
                        "error",
                        &format!("Kill pid {} failed: {}", pid, e),
                    );
                    let notifications = &shared_state.read().await.notifications;
                    notifications.add(notif).await;
                }
            }
        }
        ParsedCommand::Help => {
            let log_event = TuiEvent::SentCommand {
                cmd: "help".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            action_tx.send(Action::ToggleHelpModal).await?;
        }
        ParsedCommand::ToggleLogs => {
            let log_event = TuiEvent::SentCommand {
                cmd: "logs".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            action_tx.send(Action::ToggleLogs).await?;
        }
        ParsedCommand::ToggleNotifications => {
            let log_event = TuiEvent::SentCommand {
                cmd: "notifs".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            action_tx.send(Action::ToggleNotificationsPopup).await?;
        }
        ParsedCommand::ToggleNewAgentForm => {
            let log_event = TuiEvent::SentCommand {
                cmd: "new-agent-form".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            let new_form = if current_tui_state.new_agent_form.is_some() {
                None
            } else {
                Some(NewAgentFormState {
                    name: String::new(),
                    goal: String::new(),
                    focused_field: 0,
                    error: None,
                })
            };
            action_tx.send(Action::SetNewAgentForm(new_form)).await?;
        }
        ParsedCommand::Invalid(_) => {} // Already handled in caller
        ParsedCommand::Catalog => {
            let log_event = TuiEvent::SentCommand {
                cmd: "catalog".to_string(),
                timestamp: std::time::Instant::now(),
            };
            let _ = action_tx.send(Action::LogEvent(log_event)).await;
            // Switch to catalog tab and re-fetch
            action_tx
                .send(Action::SwitchTab(super::state::TuiTab::Catalog))
                .await?;
            if let Some(disp) = shared_state.read().await.dispatcher.clone() {
                let action_tx_c = action_tx.clone();
                tokio::spawn(async move {
                    if let Ok(agents) =
                        avix_client_core::commands::list_installed(&disp, "default").await
                    {
                        use avix_core::agent_manifest::{AgentManifestSummary, AgentScope};
                        let summaries: Vec<AgentManifestSummary> = agents
                            .iter()
                            .filter_map(|v| {
                                Some(AgentManifestSummary {
                                    name: v["name"].as_str()?.to_string(),
                                    version: v["version"].as_str().unwrap_or("?").to_string(),
                                    description: v["description"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string(),
                                    author: v["author"].as_str().unwrap_or("").to_string(),
                                    path: v["path"].as_str().unwrap_or("").to_string(),
                                    scope: if v["scope"].as_str() == Some("user") {
                                        AgentScope::User
                                    } else {
                                        AgentScope::System
                                    },
                                })
                            })
                            .collect();
                        let _ = action_tx_c.send(Action::UpdateCatalog(summaries)).await;
                    }
                });
            }
        }
    }
    Ok(())
}

// update_state_from_shared reads shared state into local TUI state.
// It must NOT make network calls — agent list refresh is handled by a
// background polling task spawned in run_app.
async fn update_state_from_shared(
    state: &mut TuiState,
    shared: &SharedState,
    _config: &ClientConfig,
) {
    let s = shared.read().await;
    state.connected = matches!(
        s.connection_status,
        avix_client_core::state::ConnectionStatus::Connected { .. }
    );
    state.agents = s.agents.read().await.clone();
    state.notifications = s.notifications.all().await;
    debug!("Notifications updated: {} total", state.notifications.len());
    state.notifications_count = state.notifications.iter().filter(|n| !n.read).count();
    state.hil_pending = state
        .notifications
        .iter()
        .filter(|n| n.kind == NotificationKind::Hil && n.hil.as_ref().unwrap().outcome.is_none())
        .count();
    debug!("HIL pending: {}", state.hil_pending);
}

pub async fn run(_json: bool) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, _json).await;
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

async fn run_app(terminal: &mut Tui, _json: bool) -> Result<()> {
    let mut state = TuiState::default();
    let client_config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
    debug!(
        "TUI config: url={} identity={}",
        client_config.server_url, client_config.identity
    );
    let shared_state = new_shared(client_config.clone());
    let (action_tx, mut action_rx) = mpsc::channel(100);

    // Load persisted notifications
    if let Ok(notifications) = persistence::load_notifications() {
        for notif in notifications {
            shared_state.read().await.notifications.add(notif).await;
        }
    }

    // Background task: refresh the agent list every 2 s without blocking the
    // main render loop.  Holds the AppState read-lock only briefly (to clone
    // the dispatcher Arc), then releases it before the network call.
    {
        let bg_shared = Arc::clone(&shared_state);
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(2_000));
            interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let (dispatcher_opt, agents_arc) = {
                    let s = bg_shared.read().await;
                    if !matches!(
                        s.connection_status,
                        avix_client_core::state::ConnectionStatus::Connected { .. }
                    ) {
                        continue;
                    }
                    (s.dispatcher.clone(), Arc::clone(&s.agents))
                }; // read lock released before network call
                if let Some(dispatcher) = dispatcher_opt {
                    if let Ok(raw) = list_agents(&dispatcher).await {
                        #[derive(serde::Deserialize)]
                        struct AgentRow {
                            pid: u64,
                            name: String,
                            #[serde(default)]
                            session_id: String,
                            status: String,
                            goal: String,
                        }
                        let updated: Vec<avix_client_core::state::ActiveAgent> = raw
                            .into_iter()
                            .filter_map(|v| {
                                serde_json::from_value::<AgentRow>(v).ok().map(|r| {
                                    avix_client_core::state::ActiveAgent {
                                        pid: r.pid,
                                        name: r.name,
                                        session_id: r.session_id,
                                        status: match r.status.as_str() {
                                            "running" => {
                                                avix_client_core::atp::types::AgentStatus::Running
                                            }
                                            _ => avix_client_core::atp::types::AgentStatus::Stopped,
                                        },
                                        goal: r.goal,
                                    }
                                })
                            })
                            .collect();
                        *agents_arc.write().await = updated;
                    }
                }
            }
        });
    }

    loop {
        // 1. Apply any queued actions from background tasks (instant).
        while let Ok(action) = action_rx.try_recv() {
            state.reducer(action);
        }

        // 2. Draw current local state immediately — form close and other local
        //    state changes are visible on this iteration, not the next.
        terminal.draw(|f| ui(f, &state))?;

        // 3. Poll for input first, BEFORE the shared-state sync.  This is the
        //    critical fix: event handling (e.g. form submit) must not be gated
        //    behind any network-touching code.
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if let Some(hil) = &state.pending_hil {
                    // Handle HIL modal keys
                    match key.code {
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            debug!("Key event: approve HIL");
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ = resolve_hil(
                                    dispatcher,
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
                            debug!("Key event: deny HIL");
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ = resolve_hil(
                                    dispatcher,
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
                            debug!("Key event: dismiss HIL");
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
                            debug!("Key event: submit new agent form");
                            // Validate form
                            let mut new_form = form.clone();
                            if form.name.trim().is_empty() {
                                new_form.error = Some("Agent name cannot be empty".into());
                            } else if form.goal.trim().is_empty() {
                                new_form.error = Some("Agent goal cannot be empty".into());
                            } else {
                                // Submit form
                                if let Some(dispatcher) =
                                    shared_state.read().await.dispatcher.clone()
                                {
                                    match spawn_agent(&dispatcher, &form.name, &form.goal, &[])
                                        .await
                                    {
                                        Ok(pid) => {
                                            debug!("Agent spawned successfully: pid={}", pid);
                                            // Add success notification
                                            let message = format!(
                                                "Agent {} spawned with PID {}",
                                                form.name, pid
                                            );
                                            let notif =
                                                Notification::from_sys_alert("info", &message);
                                            shared_state
                                                .read()
                                                .await
                                                .notifications
                                                .add(notif)
                                                .await;
                                            state.reducer(Action::SetNewAgentForm(None));
                                        }
                                        Err(e) => {
                                            new_form.error =
                                                Some(format!("Failed to spawn agent: {}", e));
                                        }
                                    }
                                } else {
                                    new_form.error = Some("No connection to server".into());
                                }
                            }
                            if new_form.error.is_some() {
                                state.reducer(Action::SetNewAgentForm(Some(new_form)));
                            }
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
                            debug!("Key event: mark notification read");
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
                } else if state.help_modal_open {
                    // Help modal mode: handle Esc to close modal
                    // See [TUI Key Bindings Reference](docs/architecture/tui.md#key-bindings).
                    if key.code == KeyCode::Esc {
                        debug!("Key event: close help modal");
                        state.reducer(Action::CloseHelpModal);
                    }
                } else if state.command_mode {
                    // Command mode
                    match key.code {
                        KeyCode::Esc => {
                            state.reducer(Action::ExitCommandMode);
                        }
                        KeyCode::Enter => {
                            if let Some(input) = &state.command_input {
                                let cmd = input.input.clone();
                                state.reducer(Action::SubmitCommand(cmd.clone()));
                                // Parse and dispatch
                                match super::parser::parse(&format!("/{}", cmd)) {
                                    Ok(parsed) => {
                                        let _ = dispatch_parsed_command(
                                            parsed,
                                            &shared_state,
                                            &client_config,
                                            &action_tx,
                                            &state,
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        let notif = Notification::from_sys_alert("error", &e);
                                        let notifications =
                                            &shared_state.read().await.notifications;
                                        let _ = notifications.add(notif).await;
                                    }
                                }
                            }
                        }
                        KeyCode::Char(c) => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::Char(c)));
                        }
                        KeyCode::Backspace => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::Backspace));
                        }
                        KeyCode::Left => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::Left));
                        }
                        KeyCode::Right => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::Right));
                        }
                        KeyCode::Up => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::HistoryUp));
                        }
                        KeyCode::Down => {
                            state.reducer(Action::UpdateCommandInput(InputDelta::HistoryDown));
                        }
                        _ => {}
                    }
                } else {
                    // Normal mode
                    match key.code {
                        KeyCode::Char('/') => {
                            state.reducer(Action::EnterCommandMode);
                        }
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('c') => {
                            if !state.connected {
                                // Ensure server is running
                                if let Err(e) = ServerHandle::ensure_running(&client_config).await {
                                    warn!("Failed to ensure local server running: {}", e);
                                    let notif = Notification::from_sys_alert(
                                        "error",
                                        &format!("Local server failed to start: {}", e),
                                    );
                                    let notifications = &shared_state.read().await.notifications;
                                    let _ = notifications.add(notif).await;
                                }
                                debug!("Connecting to WS {}", client_config.server_url);
                                match AtpClient::connect(client_config.clone()).await {
                                    Ok(client) => {
                                        let dispatcher = Arc::new(Dispatcher::new(client));
                                        {
                                            let mut s = shared_state.write().await;
                                            s.connection_status =
                                                avix_client_core::state::ConnectionStatus::Connected {
                                                    session_id: "tui-session".into(),
                                                };

                                            // Subscribe to dispatcher events directly — avoids
                                            // the Arc::try_unwrap panic in the old emitter setup.
                                            let mut rx = dispatcher.events();
                                            let state_c = Arc::clone(&shared_state);
                                            let notifications = Arc::clone(&s.notifications);
                                            let action_tx_c = action_tx.clone();
                                            tokio::spawn(async move {
                                                loop {
                                                    match rx.recv().await {
                                                        Ok(event) => {
                                                            dispatch_event(
                                                                &state_c,
                                                                &notifications,
                                                                event,
                                                                &action_tx_c,
                                                            )
                                                            .await;
                                                        }
                                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                                            warn!("Event receiver lagged by {} messages", n);
                                                        }
                                                        Err(_) => {
                                                            warn!("ATP event stream closed");
                                                            break;
                                                        }
                                                    }
                                                }
                                            });

                                            s.dispatcher = Some(dispatcher);
                                        }
                                        debug!("Connection successful");
                                        state.reducer(Action::Connect);
                                        // Fetch catalog on connect
                                        if let Some(disp) =
                                            shared_state.read().await.dispatcher.clone()
                                        {
                                            let action_tx_c = action_tx.clone();
                                            tokio::spawn(async move {
                                                if let Ok(agents) =
                                                    avix_client_core::commands::list_installed(
                                                        &disp, "default",
                                                    )
                                                    .await
                                                {
                                                    use avix_core::agent_manifest::{
                                                        AgentManifestSummary, AgentScope,
                                                    };
                                                    let summaries: Vec<AgentManifestSummary> =
                                                        agents
                                                            .iter()
                                                            .filter_map(|v| {
                                                                Some(AgentManifestSummary {
                                                                    name: v["name"]
                                                                        .as_str()?
                                                                        .to_string(),
                                                                    version: v["version"]
                                                                        .as_str()
                                                                        .unwrap_or("?")
                                                                        .to_string(),
                                                                    description: v["description"]
                                                                        .as_str()
                                                                        .unwrap_or("")
                                                                        .to_string(),
                                                                    author: v["author"]
                                                                        .as_str()
                                                                        .unwrap_or("")
                                                                        .to_string(),
                                                                    path: v["path"]
                                                                        .as_str()
                                                                        .unwrap_or("")
                                                                        .to_string(),
                                                                    scope: if v["scope"].as_str()
                                                                        == Some("user")
                                                                    {
                                                                        AgentScope::User
                                                                    } else {
                                                                        AgentScope::System
                                                                    },
                                                                })
                                                            })
                                                            .collect();
                                                    let _ = action_tx_c
                                                        .send(Action::UpdateCatalog(summaries))
                                                        .await;
                                                }
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        warn!("ATP connection failed: {}", e);
                                        let notif = Notification::from_sys_alert(
                                            "error",
                                            &format!("Connection failed: {e}"),
                                        );
                                        let _ = shared_state
                                            .read()
                                            .await
                                            .notifications
                                            .add(notif)
                                            .await;
                                    }
                                }
                            }
                        }
                        KeyCode::Char('a') => {
                            debug!("Key event: spawn test agent");
                            if let Some(dispatcher) = &shared_state.read().await.dispatcher {
                                let _ =
                                    spawn_agent(dispatcher, "test-agent", "test goal", &[]).await;
                            }
                        }
                        KeyCode::Char('f') => {
                            debug!("Key event: toggle new agent form");
                            // Toggle new agent form
                            let new_form = if state.new_agent_form.is_some() {
                                None
                            } else {
                                Some(NewAgentFormState {
                                    name: String::new(),
                                    goal: String::new(),
                                    focused_field: 0,
                                    error: None,
                                })
                            };
                            state.reducer(Action::SetNewAgentForm(new_form));
                        }
                        KeyCode::Char('n') => {
                            state.reducer(Action::ToggleNotificationsPopup);
                        }
                        KeyCode::Tab => {
                            let next_tab = match state.active_tab {
                                super::state::TuiTab::Running => super::state::TuiTab::Catalog,
                                super::state::TuiTab::Catalog => super::state::TuiTab::Running,
                            };
                            state.reducer(Action::SwitchTab(next_tab));
                        }
                        KeyCode::Up => match state.active_tab {
                            super::state::TuiTab::Catalog => {
                                state.catalog_widget.select_prev();
                            }
                            _ => {
                                state.agent_list_widget.select_prev(&state.agents);
                            }
                        },
                        KeyCode::Down => match state.active_tab {
                            super::state::TuiTab::Catalog => {
                                let items = state.catalog.clone();
                                state.catalog_widget.select_next(&items);
                            }
                            _ => {
                                state.agent_list_widget.select_next(&state.agents);
                            }
                        },
                        _ => {}
                    }
                }
            }
        }

        // 4. Sync from shared state (now fast: no network calls).
        let was_connected = state.connected;
        update_state_from_shared(&mut state, &shared_state, &client_config).await;
        if state.connected != was_connected {
            if state.connected {
                info!("Login successful");
            } else {
                info!("Disconnected");
            }
        }
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

    // If help modal open, render it
    if state.help_modal_open {
        let size = f.size();
        let modal_area = Rect {
            x: size.width / 8,
            y: size.height / 8,
            width: (size.width * 6) / 8,
            height: (size.height * 6) / 8,
        };
        let list = state.help_modal_widget.render(modal_area);
        f.render_widget(list, modal_area);
        return;
    }

    let size = f.size();

    let command_bar_height = if state.command_mode { 2 } else { 0 };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                  // status bar
            Constraint::Percentage(20),             // agents list
            Constraint::Min(10),                    // main area (log | output)
            Constraint::Length(command_bar_height), // command bar
            Constraint::Length(1),                  // notification bar
        ])
        .split(size);

    let main_area = Layout::horizontal([
        Constraint::Percentage(if state.log_visible { 30 } else { 0 }), // log
        Constraint::Percentage(100),                                    // output
    ])
    .split(layout[2]);

    // Status bar
    let status_bar = state.status_widget.render(state);
    f.render_widget(status_bar, layout[0]);

    match state.active_tab {
        super::state::TuiTab::Catalog => {
            // Catalog tab: full main area shows catalog list
            let catalog_area = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0)])
                .split(layout[2])[0];
            let catalog_list = state.catalog_widget.render(&state.catalog);
            f.render_widget(catalog_list, catalog_area);
        }
        super::state::TuiTab::Running => {
            // Running tab: agents list + output pane
            let agents_list = state.agent_list_widget.render(&state.agents, layout[1]);
            f.render_widget(agents_list, layout[1]);

            // Event log (if visible)
            if state.log_visible {
                let event_log = state
                    .event_log_widget
                    .render(&state.event_log, main_area[0]);
                f.render_widget(event_log, main_area[0]);
            }

            // Agent output pane
            let output_area = if state.log_visible {
                main_area[1]
            } else {
                layout[2]
            };
            if let Some(selected_pid) = state
                .agents
                .get(state.agent_list_widget.selected_index)
                .map(|a| a.pid)
            {
                if let Some(buf) = state.agent_output_buffers.get(&selected_pid) {
                    let output_para = buf.render(selected_pid, output_area);
                    f.render_widget(output_para, output_area);
                } else {
                    let empty = Paragraph::new("No output yet").block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!("Agent {} Output", selected_pid)),
                    );
                    f.render_widget(empty, output_area);
                }
            } else {
                let no_selection = Paragraph::new("No agent selected")
                    .block(Block::default().borders(Borders::ALL).title("Agent Output"));
                f.render_widget(no_selection, output_area);
            }
        }
    }

    // Command bar (if in command mode)
    if state.command_mode {
        let command_bar = state.command_bar_widget.render(state);
        f.render_widget(command_bar, layout[3]);
    }

    // Notifications bar
    let notifs_bar = state
        .notification_bar_widget
        .render_bar(state.notifications_count);
    f.render_widget(notifs_bar, layout[4]);
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
