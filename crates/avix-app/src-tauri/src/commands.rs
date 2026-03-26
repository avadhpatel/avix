use serde::{Deserialize, Serialize};
use tauri::State;

use avix_client_core::atp::types::HilOutcome;
use avix_client_core::commands::spawn_agent::spawn_agent as core_spawn_agent;
use avix_client_core::commands::{
    list_agents as core_list_agents, resolve_hil as core_resolve_hil,
};
use avix_client_core::state::SharedState;

#[derive(Serialize, Deserialize)]
pub struct SpawnAgentRequest {
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub async fn spawn_agent(
    state: State<'_, SharedState>,
    request: SpawnAgentRequest,
) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_spawn_agent(dispatcher, &request.name, &request.description, &[]).await {
                Ok(pid) => Ok(pid.to_string()),
                Err(e) => Err(format!("Failed to spawn agent: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[tauri::command]
pub async fn resolve_hil(
    state: State<'_, SharedState>,
    id: String,
    approve: bool,
) -> Result<(), String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            let (pid, token) = {
                let pending = s.pending_hils.read().await;
                pending.get(&id).cloned()
            }
            .ok_or("HIL not found".to_string())?;
            let outcome = if approve {
                HilOutcome::Approved
            } else {
                HilOutcome::Denied
            };
            match core_resolve_hil(dispatcher, pid, &id, &token, approve, None).await {
                Ok(_) => {
                    // Remove from pending
                    let mut pending = s.pending_hils.write().await;
                    pending.remove(&id);
                    // Update notification
                    s.notifications.resolve_hil(&id, outcome).await;
                    Ok(())
                }
                Err(e) => Err(format!("Failed to resolve HIL: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[tauri::command]
pub async fn list_agents(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    if let Some(dispatcher) = &s.dispatcher {
        if let Some(_session) = s.connection_status.session_id() {
            match core_list_agents(dispatcher).await {
                Ok(agents) => serde_json::to_string(&agents).map_err(|e| e.to_string()),
                Err(e) => Err(format!("Failed to list agents: {:?}", e)),
            }
        } else {
            Err("Not connected".to_string())
        }
    } else {
        Err("No dispatcher".to_string())
    }
}

#[tauri::command]
pub async fn get_notifications(state: State<'_, SharedState>) -> Result<String, String> {
    let s = state.read().await;
    let notifications = s.notifications.all().await;
    serde_json::to_string(&notifications).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_layout(state: State<'_, SharedState>, layout_json: String) -> Result<(), String> {
    let _s = state.read().await;
    // Parse and save
    let _: serde_json::Value =
        serde_json::from_str(&layout_json).map_err(|e| format!("Invalid JSON: {:?}", e))?;
    avix_client_core::persistence::save_json(
        &avix_client_core::persistence::layout_path(),
        &layout_json,
    )
    .map_err(|e| format!("Failed to save layout: {:?}", e))
}
