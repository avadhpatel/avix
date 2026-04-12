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
use crate::types::token::CapabilityToken;
use crate::types::Pid;

/// Kernel IPC server — listens on AVIX_KERNEL_SOCK and dispatches
/// `kernel/proc/*` requests to `ProcHandler`.
///
/// Architecture invariant: all IPC calls use a fresh connection per call (ADR-05).
/// The server reads one request per connection, sends one response, then closes.
pub struct KernelIpcServer {
    sock_path: PathBuf,
    proc_handler: Arc<ProcHandler>,
    avix_root: PathBuf,
}

impl KernelIpcServer {
    pub fn new(sock_path: PathBuf, proc_handler: Arc<ProcHandler>, avix_root: PathBuf) -> Self {
        Self {
            sock_path,
            proc_handler,
            avix_root,
        }
    }

    /// Bind the socket and start serving. Returns a handle to cancel the server.
    pub async fn start(self) -> Result<IpcServerHandle, AvixError> {
        let (server, handle) = IpcServer::bind(self.sock_path.clone()).await?;
        let path = self.sock_path.clone();
        info!(sock = %path.display(), "kernel IPC server bound");

        let proc_handler = Arc::clone(&self.proc_handler);
        let avix_root = self.avix_root;
        tokio::spawn(async move {
            if let Err(e) = server
                .serve(move |msg| {
                    let ph = Arc::clone(&proc_handler);
                    let root = avix_root.clone();
                    async move { handle_message(msg, ph, root).await }
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
    avix_root: PathBuf,
) -> Option<JsonRpcResponse> {
    match msg {
        IpcMessage::Request(req) => {
            debug!(method = %req.method, id = %req.id, "kernel IPC request");
            let resp =
                dispatch_request(&req.id, &req.method, req.params, proc_handler, avix_root).await;
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
    avix_root: PathBuf,
) -> JsonRpcResponse {
    match method {
        "kernel/proc/spawn" => {
            let name = params["name"].as_str().unwrap_or("unnamed");
            let goal = params["goal"].as_str().unwrap_or("");
            let session_id = params["session_id"].as_str().unwrap_or("");
            let caller = params["caller"].as_str().unwrap_or("gateway");
            let parent_pid = params["parent_pid"].as_u64();

            match proc_handler
                .spawn(name, goal, session_id, caller, parent_pid)
                .await
            {
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
                .unwrap_or(0);

            match method {
                "kernel/proc/kill" => {
                    // Abort the executor task first, then update the process table.
                    proc_handler.abort_agent(pid_val).await;
                    kill_proc(id, pid_val, proc_handler.process_table()).await
                }
                "kernel/proc/stat" => stat_proc(id, pid_val, proc_handler.process_table()).await,
                "kernel/proc/pause" => match proc_handler.pause_agent(pid_val).await {
                    Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                    Err(e) => JsonRpcResponse::err(id, -32000, &e.to_string(), None),
                },
                "kernel/proc/resume" => match proc_handler.resume_agent(pid_val).await {
                    Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                    Err(e) => JsonRpcResponse::err(id, -32000, &e.to_string(), None),
                },
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
            let session_id = params["session_id"].as_str().unwrap_or("");
            let result = if !session_id.is_empty() {
                proc_handler
                    .list_invocations_for_session(session_id)
                    .await
            } else {
                let username = params["username"].as_str().unwrap_or("");
                let agent_name = params["agent_name"].as_str();
                // `live` defaults to true to preserve backward compatibility:
                // callers that omit `live` get all records (including running).
                let live = params["live"].as_bool().unwrap_or(true);
                proc_handler
                    .list_invocations(username, agent_name, live)
                    .await
            };
            match result {
                Ok(records) => JsonRpcResponse::ok(id, json!(records)),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/invocation-list failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        "kernel/proc/invocation-conversation" => {
            let inv_id = params["id"].as_str().unwrap_or("");
            match proc_handler.read_invocation_conversation(inv_id).await {
                Ok(entries) => JsonRpcResponse::ok(id, json!(entries)),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/invocation-conversation failed");
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
                Ok(record) => JsonRpcResponse::ok(id, json!({ "success": true, "record": record })),
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
            let msg_val = params
                .get("message")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
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
            let part_val = params
                .get("part")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
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
                    Ok(None) => {
                        JsonRpcResponse::err(id, -32003, &format!("part {raw_id} not found"), None)
                    }
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

        "kernel/proc/session/pause" => {
            let session_id = params["session_id"].as_str().unwrap_or("");
            let uuid = match uuid::Uuid::parse_str(session_id) {
                Ok(u) => u,
                Err(_) => return JsonRpcResponse::err(id, -32002, "invalid session ID", None),
            };
            match proc_handler.get_session(&uuid).await {
                Ok(Some(session)) if session.owner_pid != 0 => {
                    // Pause via the session owner — this cascades to all other PIDs automatically.
                    match proc_handler.pause_agent(session.owner_pid).await {
                        Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                        Err(e) => {
                            warn!(error = %e, "kernel/proc/session/pause failed");
                            JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                        }
                    }
                }
                Ok(Some(_)) => {
                    // Session has no owner PID (no active agents).
                    JsonRpcResponse::err(id, -32001, "session has no active owner pid", None)
                }
                Ok(None) => JsonRpcResponse::err(
                    id,
                    -32003,
                    &format!("session {session_id} not found"),
                    None,
                ),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/session/pause failed");
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
            // If the session is Paused with active PIDs, send SIGRESUME to all PIDs rather
            // than spawning a new invocation.
            match proc_handler.get_session(&uuid).await {
                Ok(Some(session))
                    if matches!(session.status, crate::session::SessionStatus::Paused)
                        && !session.pids.is_empty() =>
                {
                    let pids = session.pids.clone();
                    for pid in pids {
                        let _ = proc_handler.resume_agent(pid).await;
                    }
                    JsonRpcResponse::ok(id, json!({ "ok": true }))
                }
                _ => {
                    // Idle/Running session — spawn a new invocation (existing path).
                    match proc_handler.resume_session(&uuid, input).await {
                        Ok(pid) => JsonRpcResponse::ok(id, json!({ "pid": pid })),
                        Err(e) => {
                            warn!(error = %e, "kernel/proc/session/resume failed");
                            JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                        }
                    }
                }
            }
        }

        "kernel/proc/package/install-agent" => {
            debug!("handling kernel/proc/package/install-agent");
            let ctx = crate::syscall::SyscallContext {
                caller_pid: 0,
                token: CapabilityToken::test_token(&["proc/package/install-agent"]),
            };
            let result =
                crate::syscall::domain::pkg_::install_agent(&ctx, params, &avix_root).await;
            match result {
                Ok(v) => JsonRpcResponse::ok(id, v),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/package/install-agent failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }
        "kernel/proc/package/uninstall-agent" => {
            debug!("handling kernel/proc/package/uninstall-agent");
            let ctx = crate::syscall::SyscallContext {
                caller_pid: 0,
                token: CapabilityToken::test_token(&["proc/package/install-agent"]),
            };
            let root = avix_root.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::syscall::domain::pkg_::uninstall_agent(&ctx, params.clone(), &root)
            })
            .await;
            match result {
                Ok(Ok(v)) => JsonRpcResponse::ok(id, v),
                Ok(Err(e)) => {
                    warn!(error = %e, "kernel/proc/package/uninstall-agent failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
                Err(e) => {
                    warn!(error = %e, "kernel/proc/package/uninstall-agent task failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }
        "kernel/proc/package/install-service" => {
            debug!("handling kernel/proc/package/install-service");
            let ctx = crate::syscall::SyscallContext {
                caller_pid: 0,
                token: CapabilityToken::test_token(&["proc/package/install-service"]),
            };
            let result =
                crate::syscall::domain::pkg_::install_service(&ctx, params, &avix_root).await;
            match result {
                Ok(v) => JsonRpcResponse::ok(id, v),
                Err(e) => {
                    warn!(error = %e, "kernel/proc/package/install-service failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }
        "kernel/proc/package/uninstall-service" => {
            debug!("handling kernel/proc/package/uninstall-service");
            let ctx = crate::syscall::SyscallContext {
                caller_pid: 0,
                token: CapabilityToken::test_token(&["proc/package/install-service"]),
            };
            let root = avix_root.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::syscall::domain::pkg_::uninstall_service(&ctx, params.clone(), &root)
            })
            .await;
            match result {
                Ok(Ok(v)) => JsonRpcResponse::ok(id, v),
                Ok(Err(e)) => {
                    warn!(error = %e, "kernel/proc/package/uninstall-service failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
                Err(e) => {
                    warn!(error = %e, "kernel/proc/package/uninstall-service task failed");
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

        "kernel/signal/send" => {
            let pid_val = params["pid"]
                .as_u64()
                .unwrap_or(0);
            if pid_val == 0 {
                return JsonRpcResponse::err(id, -32602, "missing pid", None);
            }

            let signal = params["signal"].as_str().unwrap_or("").to_string();
            if signal.is_empty() {
                return JsonRpcResponse::err(id, -32602, "missing signal", None);
            }

            let payload = params["payload"].clone();
            match proc_handler.send_signal(pid_val, &signal, payload).await {
                Ok(()) => JsonRpcResponse::ok(id, json!({ "ok": true })),
                Err(e) => {
                    warn!(pid = pid_val, signal, error = %e, "kernel/signal/send failed");
                    JsonRpcResponse::err(id, -32000, &e.to_string(), None)
                }
            }
        }

        other => {
            warn!(method = other, "kernel IPC: unknown method");
            JsonRpcResponse::err(id, -32601, &format!("unknown kernel method: {other}"), None)
        }
    }
}

async fn kill_proc(id: &str, pid: u64, table: &Arc<ProcessTable>) -> JsonRpcResponse {
    match table
        .set_status(Pid::from_u64(pid), ProcessStatus::Stopped)
        .await
    {
        Ok(_) => {
            info!(pid, "agent killed via IPC");
            JsonRpcResponse::ok(id, json!({ "ok": true }))
        }
        Err(e) => JsonRpcResponse::err(id, -32003, &e.to_string(), None),
    }
}

async fn stat_proc(id: &str, pid: u64, table: &Arc<ProcessTable>) -> JsonRpcResponse {
    match table.get(Pid::from_u64(pid)).await {
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

    fn make_avix_root(dir: &TempDir) -> PathBuf {
        dir.path().to_path_buf()
    }

    #[tokio::test]
    async fn spawn_returns_pid() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "test-agent", "goal": "do stuff", "session_id": "s1", "caller": "gw" }),
            ph,
            root,
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
        let root = make_avix_root(&dir);

        // Spawn one agent first
        dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "a1", "goal": "g1", "session_id": "s1", "caller": "gw" }),
            Arc::clone(&ph),
            root.clone(),
        )
        .await;

        let resp = dispatch_request("req-2", "kernel/proc/list", json!({}), ph, root).await;
        assert!(resp.error.is_none());
        let list = resp.result.unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stat_returns_agent_info() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);

        let spawn_resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "agent-x", "goal": "my-goal", "session_id": "s1", "caller": "gw" }),
            Arc::clone(&ph),
            root.clone(),
        )
        .await;
        let pid = spawn_resp.result.unwrap()["pid"].as_u64().unwrap();

        let stat_resp =
            dispatch_request("req-2", "kernel/proc/stat", json!({ "id": pid }), ph, root).await;
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
        let root = make_avix_root(&dir);

        let spawn_resp = dispatch_request(
            "req-1",
            "kernel/proc/spawn",
            json!({ "name": "doomed", "goal": "g", "session_id": "s", "caller": "gw" }),
            Arc::clone(&ph),
            root.clone(),
        )
        .await;
        let pid = spawn_resp.result.unwrap()["pid"].as_u64().unwrap();

        let kill_resp = dispatch_request(
            "req-2",
            "kernel/proc/kill",
            json!({ "id": pid }),
            Arc::clone(&ph),
            root.clone(),
        )
        .await;
        assert!(kill_resp.error.is_none());

        // Verify status is now stopped
        let stat_resp =
            dispatch_request("req-3", "kernel/proc/stat", json!({ "id": pid }), ph, root).await;
        assert_eq!(stat_resp.result.unwrap()["status"], "stopped");
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request("req-1", "kernel/bogus/method", json!({}), ph, root).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // T-GW-10
    #[tokio::test]
    async fn list_installed_returns_ok() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/list-installed",
            json!({ "username": "alice" }),
            ph,
            root,
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
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-list",
            json!({ "username": "alice" }),
            ph,
            root,
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
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-get",
            json!({ "id": "does-not-exist" }),
            ph,
            root,
        )
        .await;
        // No store configured → Ok(None) → 404-style error
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32003);
    }

    // T-GW-13a
    #[tokio::test]
    async fn invocation_conversation_returns_empty_for_unknown_id() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-conversation",
            json!({ "id": "no-such-id" }),
            ph,
            root,
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 0);
    }

    // T-GW-13b
    #[tokio::test]
    async fn invocation_list_with_session_id_returns_empty_without_store() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request(
            "req-1",
            "kernel/proc/invocation-list",
            json!({ "session_id": "sess-xyz" }),
            ph,
            root,
        )
        .await;
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap().as_array().unwrap().len(), 0);
    }

    // T-GW-13: regression — unknown op must NOT match new ops
    #[tokio::test]
    async fn unknown_method_still_returns_eparse_after_new_ops() {
        let dir = TempDir::new().unwrap();
        let ph = make_proc_handler(&dir);
        let root = make_avix_root(&dir);
        let resp = dispatch_request("req-1", "kernel/proc/bogus-new-op", json!({}), ph, root).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
