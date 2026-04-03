use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tracing::{debug, info, warn};

use crate::error::AvixError;
use crate::ipc::message::{IpcMessage, JsonRpcResponse};
use crate::ipc::{IpcServer, IpcServerHandle};
use crate::kernel::proc::ProcHandler;
use crate::process::entry::ProcessStatus;
use crate::process::table::ProcessTable;
use crate::types::Pid;

/// Kernel IPC server — listens on AVIX_KERNEL_SOCK and dispatches
/// `kernel/proc/*` requests to `ProcHandler`.
///
/// Architecture invariant: all IPC calls use a fresh connection per call (ADR-05).
/// The server reads one request per connection, sends one response, then closes.
pub struct KernelIpcServer {
    sock_path: PathBuf,
    proc_handler: Arc<ProcHandler>,
}

impl KernelIpcServer {
    pub fn new(sock_path: PathBuf, proc_handler: Arc<ProcHandler>) -> Self {
        Self {
            sock_path,
            proc_handler,
        }
    }

    /// Bind the socket and start serving. Returns a handle to cancel the server.
    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        let path = self.sock_path.clone();
        info!(sock = %path.display(), "kernel IPC server bound");

        let proc_handler = Arc::clone(&self.proc_handler);
        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let ph = Arc::clone(&proc_handler);
                    async move { handle_message(msg, ph).await }
                })
                .await
            {
                warn!(error = %e, "kernel IPC server exited");
            }
        });

        Ok(handle)
    }
}

/// Route one IPC message to the appropriate kernel/proc handler.
async fn handle_message(
    msg: IpcMessage,
    proc_handler: Arc<ProcHandler>,
) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => {
            debug!(method = %req.method, id = %req.id, "kernel IPC request");
            let resp = dispatch_request(&req.id, &req.method, req.params, proc_handler).await;
            Some(resp)
        }
        IpcMessage::Notification(notif) => {
            debug!(method = %notif.method, "kernel IPC notification (ignored)");
            None
        }
    }
}

async fn dispatch_request(
    id: &str,
    method: &str,
    params: serde_json::Value,
    proc_handler: Arc<ProcHandler>,
) -> JsonRpcResponse {
    match method {
        "kernel/proc/spawn" => {
            let name = params["name"].as_str().unwrap_or("unnamed");
            let goal = params["goal"].as_str().unwrap_or("");
            let session_id = params["session_id"].as_str().unwrap_or("unknown");
            let caller = params["caller"].as_str().unwrap_or("gateway");

            match proc_handler.spawn(name, goal, session_id, caller).await {
                Ok(pid) => {
                    info!(pid, name, "agent spawned via IPC");
                    JsonRpcResponse::ok(id, json!({ "pid": pid, "status": "running" }))
                }
                Err(e) => {
                    warn!(error = %e, "kernel/proc/spawn failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/list" => match proc_handler.list().await {
            Ok(agents) => {
                let list: Vec<_> = agents
                    .into_iter()
                    .map(|a| {
                        json!({
                            "pid": a.pid,
                            "name": a.name,
                            "status": a.status,
                            "goal": a.goal,
                        })
                    })
                    .collect();
                JsonRpcResponse::ok(id, json!(list))
            }
            Err(e) => {
                warn!(error = %e, "kernel/proc/list failed");
                JsonRpcResponse::err(id, -32000, &e.to_string(), None)
            }
        },

        "kernel/proc/kill" | "kernel/proc/stat" | "kernel/proc/pause" | "kernel/proc/resume"
        | "kernel/proc/wait" | "kernel/proc/setcap" => {
            let pid_val = params["id"]
                .as_u64()
                .or_else(|| params["pid"].as_u64())
                .unwrap_or(0) as u32;

            match method {
                "kernel/proc/kill" => {
                    // Abort the executor task first, then update the process table.
                    proc_handler.abort_agent(pid_val).await;
                    kill_proc(id, pid_val, proc_handler.process_table()).await
                }
                "kernel/proc/stat" => stat_proc(id, pid_val, proc_handler.process_table()).await,
                "kernel/proc/pause" => {
                    set_status(
                        id,
                        pid_val,
                        ProcessStatus::Paused,
                        proc_handler.process_table(),
                    )
                    .await
                }
                "kernel/proc/resume" => {
                    set_status(
                        id,
                        pid_val,
                        ProcessStatus::Running,
                        proc_handler.process_table(),
                    )
                    .await
                }
                // wait and setcap are stubs for now
                _ => JsonRpcResponse::ok(id, json!({ "ok": true })),
            }
        }

        "kernel/proc/list-installed" => {
            let username = params["username"].as_str().unwrap_or("");
            let summaries = proc_handler.list_installed(username).await;
            JsonRpcResponse::ok(id, json!(summaries))
        }

        "kernel/proc/invocation-list" => {
            let username = params["username"].as_str().unwrap_or("");
            let agent_name = params["agent_name"].as_str();
            // `live` defaults to true to preserve backward compatibility:
            // callers that omit `live` get all records (including running).
            let live = params["live"].as_bool().unwrap_or(true);
            match proc_handler.list_invocations(username, agent_name, live).await {
                Ok(records) => JsonRpcResponse::ok(id, json!(records)),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/invocation-list failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/invocation-get" => {
            let inv_id = params["id"].as_str().unwrap_or("");
            match proc_handler.get_invocation(inv_id).await {
                Ok(Some(record)) => JsonRpcResponse::ok(id, json!(record)),
                Ok(None) => JsonRpcResponse::err(
                    id,
                    -32003,
                    &format!("invocation {inv_id} not found"),
                    None,
                ),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/invocation-get failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/invocation-snapshot" => {
            let inv_id = params["id"].as_str().unwrap_or("");
            match proc_handler.snapshot_invocation(inv_id).await {
                Ok(record) => JsonRpcResponse::ok(
                    id,
                    json!({ "success": true, "record": record }),
                ),
                Err(e @ AvixError::NotFound(_)) => {
                    warn!(error = %e, "kernel/proc/invocation-snapshot: not found");
                    JsonRpcResponse::err(id, -32003, &e.to_string(), None)
                }
                Err(e @ AvixError::InvalidInput(_)) => {
                    warn!(error = %e, "kernel/proc/invocation-snapshot: invalid state");
                    JsonRpcResponse::err(id, -32001, &e.to_string(), None)
                }
                Err(e) => {
                    warn!(error = %e, "kernel/proc/invocation-snapshot failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        // ── History: message operations ──────────────────────────────────────

        "kernel/proc/message-create" => {
            let msg_val = params.get("message").cloned().unwrap_or(serde_json::Value::Null);
            match serde_json::from_value::<crate::history::record::MessageRecord>(msg_val) {
                Ok(msg) => match proc_handler.create_message(&msg).await {
                    Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/message-create failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(e) => {
                    warn!(error = %e, "kernel/proc/message-create: invalid message body");
                    JsonRpcResponse::err(id, -32002, &format!("invalid message: {e}"), None)
                }
            }
        }

        "kernel/proc/message-get" => {
            let raw_id = params["id"].as_str().unwrap_or("");
            match uuid::Uuid::parse_str(raw_id) {
                Ok(uuid) => match proc_handler.get_message(&uuid).await {
                    Ok(Some(msg)) => JsonRpcResponse::ok(id, json!(msg)),
                    Ok(None) => JsonRpcResponse::err(
                        id,
                        -32003,
                        &format!("message {raw_id} not found"),
                        None,
                    ),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/message-get failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(_) => JsonRpcResponse::err(id, -32002, "invalid message UUID", None),
            }
        }

        "kernel/proc/message-list" => {
            let raw_id = params["session_id"].as_str().unwrap_or("");
            match uuid::Uuid::parse_str(raw_id) {
                Ok(uuid) => match proc_handler.list_messages(&uuid).await {
                    Ok(messages) => JsonRpcResponse::ok(id, json!(messages)),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/message-list failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(_) => JsonRpcResponse::err(id, -32002, "invalid session UUID", None),
            }
        }

        // ── History: part operations ─────────────────────────────────────────

        "kernel/proc/part-create" => {
            let part_val = params.get("part").cloned().unwrap_or(serde_json::Value::Null);
            match serde_json::from_value::<crate::history::record::PartRecord>(part_val) {
                Ok(part) => match proc_handler.create_part(&part).await {
                    Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/part-create failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(e) => {
                    warn!(error = %e, "kernel/proc/part-create: invalid part body");
                    JsonRpcResponse::err(id, -32002, &format!("invalid part: {e}"), None)
                }
            }
        }

        "kernel/proc/part-get" => {
            let raw_id = params["id"].as_str().unwrap_or("");
            match uuid::Uuid::parse_str(raw_id) {
                Ok(uuid) => match proc_handler.get_part(&uuid).await {
                    Ok(Some(part)) => JsonRpcResponse::ok(id, json!(part)),
                    Ok(None) => JsonRpcResponse::err(
                        id,
                        -32003,
                        &format!("part {raw_id} not found"),
                        None,
                    ),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/part-get failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(_) => JsonRpcResponse::err(id, -32002, "invalid part UUID", None),
            }
        }

        "kernel/proc/part-list" => {
            let raw_id = params["message_id"].as_str().unwrap_or("");
            match uuid::Uuid::parse_str(raw_id) {
                Ok(uuid) => match proc_handler.list_parts(&uuid).await {
                    Ok(parts) => JsonRpcResponse::ok(id, json!(parts)),
                    Err(e) => {
                        warn!(error = %e, "kernel/proc/part-list failed");
                        JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                    }
                },
                Err(_) => JsonRpcResponse::err(id, -32002, "invalid message UUID", None),
            }
        }

        // Session operations
        "kernel/proc/session/create" => {
            let username = params["username"].as_str().unwrap_or("");
            let origin_agent = params["origin_agent"].as_str().unwrap_or("agent");
            let title = params["title"].as_str().unwrap_or("New Session");
            let goal = params["goal"].as_str().unwrap_or("");
            match proc_handler
                .create_session(username, origin_agent, title, goal)
                .await
            {
                Ok(record) => {
                    JsonRpcResponse::ok(id, json!({ "session_id": record.id.to_string() }))
                }
                Err(e) => {
                    warn!(error = %e, "kernel/proc/session/create failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/session/list" => {
            let username = params["username"].as_str().unwrap_or("");
            match proc_handler.list_sessions(username).await {
                Ok(sessions) => JsonRpcResponse::ok(id, json!(sessions)),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/session/list failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/session/get" => {
            let session_id = params["id"].as_str().unwrap_or("");
            let uuid = match uuid::Uuid::parse_str(session_id) {
                Ok(u) => u,
                Err(_) => return JsonRpcResponse::err(id, -32002, "invalid session ID", None),
            };
            match proc_handler.get_session(&uuid).await {
                Ok(Some(session)) => JsonRpcResponse::ok(id, json!(session)),
                Ok(None) => JsonRpcResponse::err(
                    id,
                    -32003,
                    &format!("session {session_id} not found"),
                    None,
                ),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/session/get failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/session/resume" => {
            let session_id = params["session_id"].as_str().unwrap_or("");
            let input = params["input"].as_str();
            let uuid = match uuid::Uuid::parse_str(session_id) {
                Ok(u) => u,
                Err(_) => return JsonRpcResponse::err(id, -32002, "invalid session ID", None),
            };
            match proc_handler.resume_session(&uuid, input).await {
                Ok(pid) => JsonRpcResponse::ok(id, json!({ "pid": pid })),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/session/resume failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/sys/service-list" => {
            debug!("handling kernel/sys/service-list");
            let response = proc_handler.list_services().await;
            debug!(
                total = response.total,
                running = response.running,
                "service-list response"
            );
            JsonRpcResponse::ok(id, serde_json::json!(response))
        }

        "kernel/sys/tool-list" => {
            debug!("handling kernel/sys/tool-list");
            let response = proc_handler.list_tools().await;
            debug!(
                total = response.total,
                available = response.available,
                "tool-list response"
            );
            JsonRpcResponse::ok(id, serde_json::json!(response))
        }

        other => {
            warn!(method = other, "kernel IPC: unknown method");
            JsonRpcResponse::err(id, -32601, &format!("unknown kernel method: {other}"), None)
        }
    }
}

async fn kill_proc(id: &str, pid: u32, table: &Arc<ProcessTable>) -> JsonRpcResponse {
    match table
        .set_status(Pid::new(pid), ProcessStatus::Stopped)
        .await
    {
        Ok(_) => {
            info!(pid, "agent killed via IPC");
            JsonRpcResponse::ok(id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::err(id, -32003, &e.to_string(), None),
    }
}

async fn stat_proc(id: &str, pid: u32, table: &Arc<ProcessTable>) -> JsonRpcResponse {
    match table.get(Pid::new(pid)).await {
        Some(entry) => {
            let status = match entry.status {
                ProcessStatus::Running => "running",
                ProcessStatus::Paused => "paused",
                ProcessStatus::Waiting => "waiting",
                ProcessStatus::Stopped => "stopped",
                ProcessStatus::Crashed => "crashed",
                ProcessStatus::Pending => "pending",
            };
            JsonRpcResponse::ok(
                id,
                json!({
                    "pid": pid,
                    "name": entry.name,
                    "status": status,
                    "goal": entry.goal,
                }),
            )
        }
        None => JsonRpcResponse::err(id, -32003, &format!("pid {pid} not found"), None),
    }
}

async fn set_status(
    id: &str,
    pid: u32,
    status: ProcessStatus,
    table: &Arc<ProcessTable>,
) -> JsonRpcResponse {
    match table.set_status(Pid::new(pid), status).await {
        Ok(_) => JsonRpcResponse::ok(id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::err(id, -32003, &e.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_proc_handler(dir: &TempDir) -> Arc<ProcHandler> {
        let table = Arc::new(ProcessTable::new());
        let yaml_path = dir.path().join("agents.yaml");
        let master_key = b"test-master-key-32-bytes-padded!".to_vec();
        Arc::new(ProcHandler::new(table, yaml_path, master_key))
    }

    #[tokio::test]
    async fn spawn_returns_pid() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "test-agent", "goal": "do stuff", "session_id": "s1", "caller": "gw" }),
            ph,
        )
        .await;
        assert!(resp.error.is_none());
        let pid = resp.result.unwrap()["pid"].as_u64().unwrap();
        assert!(pid > 0);
    }

    #[tokio::test]
    async fn list_returns_empty_then_one() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);

        // Spawn one agent first
        dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "a1", "goal": "g1", "session_id": "s1", "caller": "gw" }),
            Arc::clone(&ph),
        )
        .await;

        let resp = dispatch_request("req-2", "kernel/proc/list", json!({}), ph).await;
        assert!(resp.error.is_none());
        let list = resp.result.unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stat_returns_agent_info() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);

        let spawn_resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "agent-x", "goal": "my-goal", "session_id": "s1", "caller": "gw" }),
            Arc::clone(&ph),
        )
        .await;
        let pid = spawn_resp.result.unwrap()["pid"].as_u64().unwrap();

        let stat_resp =
            dispatch_request("req-2", "kernel/proc/stat", json!({ "id": pid }), ph).await;
        assert!(stat_resp.error.is_none());
        let body = stat_resp.result.unwrap();
        assert_eq!(body["name"], "agent-x");
        assert_eq!(body["goal"], "my-goal");
        assert_eq!(body["status"], "running");
    }

    #[tokio::test]
    async fn kill_stops_agent() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);

        let spawn_resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "doomed", "goal": "g", "session_id": "s", "caller": "gw" }),
            Arc::clone(&ph),
        )
        .await;
        let pid = spawn_resp.result.unwrap()["pid"].as_u64().unwrap();

        let kill_resp = dispatch_request(
            "req-2",
            "kernel/proc/kill",
            json!({ "id": pid }),
            Arc::clone(&ph),
        )
        .await;
        assert!(kill_resp.error.is_none());

        // Verify status is now stopped
        let stat_resp =
            dispatch_request("req-3", "kernel/proc/stat", json!({ "id": pid }), ph).await;
        assert_eq!(stat_resp.result.unwrap()["status"], "stopped");
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request("req-1", "kernel/bogus/method", json!({}), ph).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // T-GW-10
    #[tokio::test]
    async fn list_installed_returns_ok() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/list-installed",
            json!({ "username": "alice" }),
            ph,
        )
        .await;
        // No scanner configured → empty array, still OK
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 0);
    }

    // T-GW-11
    #[tokio::test]
    async fn invocation_list_returns_ok() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-list",
            json!({ "username": "alice" }),
            ph,
        )
        .await;
        // No store configured → empty array, still OK
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 0);
    }

    // T-GW-12
    #[tokio::test]
    async fn invocation_get_returns_not_found_for_unknown_id() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-get",
            json!({ "id": "does-not-exist" }),
            ph,
        )
        .await;
        // No store configured → Ok(None) → 404-style error
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32003);
    }

    // T-GW-13: regression — unknown op must NOT match new ops
    #[tokio::test]
    async fn unknown_method_still_returns_eparse_after_new_ops() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let resp = dispatch_request("req-1", "kernel/proc/bogus-new-op", json!({}), ph).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
