use avix_client_core::config::ClientConfig;
use avix_client_core::state::new_shared;
use std::sync::Arc;
use tauri::{Emitter, Manager};

mod commands;

pub async fn create_app_state(
) -> Result<avix_client_core::state::SharedState, Box<dyn std::error::Error>> {
    let config = ClientConfig::load().unwrap_or_else(|_| ClientConfig::default());
    // Auto-gen client.json if it didn't exist
    let config_path = avix_client_core::persistence::app_data_dir().join("client.json");
    if !config_path.exists() {
        avix_client_core::persistence::save_json(&config_path, &config)?;
        tracing::info!("Auto-generated client.json at {}", config_path.display());
    }

    // Embed avix daemon
    let root = config
        .runtime_root
        .clone()
        .unwrap_or_else(|| avix_client_core::persistence::app_data_dir().join("runtime"));
    let runtime = avix_core::bootstrap::Runtime::bootstrap_with_root(&root).await?;
    tokio::spawn(async move {
        if let Err(e) = runtime.start_daemon(9142, false).await {
            tracing::error!("Failed to start embedded daemon: {}", e);
        }
    });

    let state = new_shared(config);
    {
        let mut guard = state.write().await;
        if let Err(e) = guard.init().await {
            tracing::warn!("State init error (will show login): {e}");
        }
    }
    Ok(state)
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    tracing::info!("avix-app starting");

    let app_state = create_app_state().await?;

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .invoke_handler(tauri::generate_handler![
            commands::auth_status,
            commands::login,
            commands::spawn_agent,
            commands::resolve_hil,
            commands::list_agents,
            commands::pipe_text,
            commands::get_notifications,
            commands::save_layout
        ])
        .setup(|app| {
            // Set the emit callback
            let app_handle = app.handle().clone();
            let state = app.state::<avix_client_core::state::SharedState>();
            let mut state_guard = state.try_write().unwrap();
            let callback = Arc::new(move |event: &str, data: &serde_json::Value| {
                let _ = app_handle.emit(event, data);
            });
            state_guard.set_emit_callback(callback);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_app_state_with_default_config() {
        let mut config = ClientConfig::default();
        config.auto_start_server = false;
        let state = new_shared(config);
        let s = state.try_read().unwrap();
        assert_eq!(s.config.server_url, "http://localhost:9142");
        assert_eq!(
            s.connection_status,
            avix_client_core::state::ConnectionStatus::Disconnected
        );
    }
}
