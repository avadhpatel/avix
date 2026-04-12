use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::auth::atp_token::ATPTokenStore;
use crate::auth::service::AuthService;
use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::atp::types::AtpDomain;
use crate::gateway::validator::ValidatedCmd;
use crate::ipc::{message::JsonRpcRequest, IpcClient};
use crate::kernel::HilManager;

pub mod auth;
pub mod cap;
pub mod crews;
pub mod cron;
pub mod fs;
pub mod pipe;
pub mod proc;
pub mod session;
pub mod signal;
pub mod snap;
pub mod sys;
pub mod users;

/// Abstraction over kernel IPC calls (ADR-05: one fresh connection per call).
#[async_trait]
pub trait IpcRouter: Send + Sync {
    async fn call(&self, method: &str, params: Value) -> Result<Value, AtpError>;
}

/// Live implementation that connects to the kernel socket.
pub struct LiveIpcRouter {
    client: IpcClient,
}

impl LiveIpcRouter {
    pub fn new(client: IpcClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IpcRouter for LiveIpcRouter {
    async fn call(&self, method: &str, params: Value) -> Result<Value, AtpError> {
        let span = tracing::trace_span!("ipc.call", method = %method);
        let _enter = span.enter();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };
        drop(_enter); // drop before await
        let resp = self
            .client
            .call(req)
            .await
            .map_err(|e| AtpError::new(AtpErrorCode::Eunavail, e.to_string()))?;
        if let Some(err) = resp.error {
            let code = ipc_code_to_atp(&err.message, err.code);
            return Err(AtpError::new(code, err.message));
        }
        Ok(resp.result.unwrap_or(Value::Null))
    }
}

/// Null router — always returns Eunavail. Used when no kernel socket is configured.
pub struct NullIpcRouter;

#[async_trait]
impl IpcRouter for NullIpcRouter {
    async fn call(&self, _method: &str, _params: Value) -> Result<Value, AtpError> {
        Err(AtpError::new(
            AtpErrorCode::Eunavail,
            "kernel IPC not configured",
        ))
    }
}

/// In-memory proc entry tracked by TestIpcRouter.
struct TestProc {
    pid: u64,
    name: String,
    goal: String,
    is_agent: bool,
}

/// Test IPC router — simulates kernel responses for testing, maintains in-memory proc state, emits events.
/// Links: docs/dev_plans/ATP-WS-TESTS-PLAN.md#51
pub struct TestIpcRouter {
    event_bus: Arc<crate::gateway::event_bus::AtpEventBus>,
    procs: Arc<Mutex<HashMap<u64, TestProc>>>,
}

impl TestIpcRouter {
    pub fn new(event_bus: Arc<crate::gateway::event_bus::AtpEventBus>) -> Self {
        Self {
            event_bus,
            procs: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl IpcRouter for TestIpcRouter {
    async fn call(&self, method: &str, params: Value) -> Result<Value, AtpError> {
        use crate::gateway::atp::types::AtpEventKind;
        use crate::types::Role;

        tracing::debug!(method, "test IPC call");

        match method {
            "kernel/proc/spawn" => {
                let pid = crate::types::Pid::generate().as_u64();
                let is_agent = params["agent"].is_string();
                let name = if is_agent {
                    params["agent"].as_str().unwrap_or("agent").to_string()
                } else {
                    params["name"].as_str().unwrap_or("proc").to_string()
                };
                let goal = params["goal"].as_str().unwrap_or("").to_string();
                let cmd: Vec<Value> = params["cmd"].as_array().cloned().unwrap_or_default();

                // Store proc (release lock before publishing events)
                {
                    let mut procs = self.procs.lock().await;
                    procs.insert(
                        pid,
                        TestProc {
                            pid,
                            name: name.clone(),
                            goal: goal.clone(),
                            is_agent,
                        },
                    );
                }

                // Emit start event
                let (event_kind, event_body) = if is_agent {
                    (
                        AtpEventKind::AgentSpawned,
                        serde_json::json!({ "pid": pid.to_string().to_string(), "name": name, "goal": goal }),
                    )
                } else {
                    (
                        AtpEventKind::ProcStart,
                        serde_json::json!({ "pid": pid.to_string().to_string(), "cmd": cmd, "name": name }),
                    )
                };
                self.event_bus.publish(
                    crate::gateway::atp::frame::AtpEvent::new(
                        event_kind,
                        "test-session",
                        event_body,
                    ),
                    None,
                    Role::User,
                );

                // For echo procs, also emit output event (not exit — explicit kill emits exit)
                let cmd_strs: Vec<&str> = cmd.iter().filter_map(|v| v.as_str()).collect();
                if cmd_strs.first().copied() == Some("echo") {
                    let output = cmd_strs.get(1).copied().unwrap_or("");
                    self.event_bus.publish(
                        crate::gateway::atp::frame::AtpEvent::new(
                            AtpEventKind::ProcOutput,
                            "test-session",
                            serde_json::json!({ "pid": pid.to_string().to_string(), "text": output }),
                        ),
                        None,
                        Role::User,
                    );
                }

                Ok(serde_json::json!({ "pid": pid.to_string().to_string(), "status": "running" }))
            }

            "kernel/proc/list" => {
                let procs = self.procs.lock().await;
                let list: Vec<Value> = procs
                    .values()
                    .map(|p| {
                        serde_json::json!({
                            "pid": p.pid,
                            "name": p.name,
                            "goal": p.goal,
                            "status": "running",
                        })
                    })
                    .collect();
                Ok(serde_json::json!(list))
            }

            "kernel/proc/kill" => {
                let pid = params["id"]
                    .as_u64()
                    .or_else(|| params["pid"].as_u64())
                    .unwrap_or(0);
                let is_agent = {
                    let mut procs = self.procs.lock().await;
                    procs.remove(&pid).map(|p| p.is_agent).unwrap_or(false)
                };
                let (event_kind, event_body) = if is_agent {
                    (
                        crate::gateway::atp::types::AtpEventKind::AgentExit,
                        serde_json::json!({ "pid": pid.to_string().to_string(), "exitCode": 0 }),
                    )
                } else {
                    (
                        crate::gateway::atp::types::AtpEventKind::ProcExit,
                        serde_json::json!({ "pid": pid.to_string().to_string(), "exitCode": 0 }),
                    )
                };
                self.event_bus.publish(
                    crate::gateway::atp::frame::AtpEvent::new(
                        event_kind,
                        "test-session",
                        event_body,
                    ),
                    None,
                    crate::types::Role::User,
                );
                Ok(serde_json::json!({ "ok": true }))
            }

            "kernel/proc/stat" => {
                let pid = params["id"]
                    .as_u64()
                    .or_else(|| params["pid"].as_u64())
                    .unwrap_or(0);
                let procs = self.procs.lock().await;
                match procs.get(&pid) {
                    Some(p) => Ok(serde_json::json!({
                        "pid": p.pid,
                        "name": p.name,
                        "goal": p.goal,
                        "status": "running",
                    })),
                    None => Err(AtpError::new(
                        AtpErrorCode::Enotfound,
                        format!("proc {pid} not found"),
                    )),
                }
            }

            _ => Err(AtpError::new(
                AtpErrorCode::Eunavail,
                format!("test IPC: no handler for '{method}'"),
            )),
        }
    }
}

fn ipc_code_to_atp(msg: &str, code: i32) -> AtpErrorCode {
    match code {
        -32003 => AtpErrorCode::Enotfound,
        -32002 => AtpErrorCode::Eperm,
        _ => {
            if msg.contains("not found") || msg.contains("ENOENT") {
                AtpErrorCode::Enotfound
            } else if msg.contains("permission") || msg.contains("EPERM") {
                AtpErrorCode::Eperm
            } else if msg.contains("conflict") || msg.contains("ECONFLICT") {
                AtpErrorCode::Econflict
            } else if msg.contains("unavailable") || msg.contains("EUNAVAIL") {
                AtpErrorCode::Eunavail
            } else {
                AtpErrorCode::Einternal
            }
        }
    }
}

/// Runtime context shared across all domain handlers.
pub struct HandlerCtx {
    pub ipc: Arc<dyn IpcRouter>,
    pub token_store: Arc<ATPTokenStore>,
    pub auth_svc: Arc<AuthService>,
    pub hil_manager: Option<Arc<HilManager>>,
}

/// Route a validated command to the correct domain handler.
pub async fn dispatch(cmd: ValidatedCmd, ctx: &HandlerCtx) -> AtpReply {
    match cmd.cmd.domain {
        AtpDomain::Auth => auth::handle(cmd, ctx).await,
        AtpDomain::Proc => proc::handle(cmd, ctx).await,
        AtpDomain::Signal => signal::handle(cmd, ctx).await,
        AtpDomain::Fs => fs::handle(cmd, ctx).await,
        AtpDomain::Snap => snap::handle(cmd, ctx).await,
        AtpDomain::Cron => cron::handle(cmd, ctx).await,
        AtpDomain::Users => users::handle(cmd, ctx).await,
        AtpDomain::Crews => crews::handle(cmd, ctx).await,
        AtpDomain::Cap => cap::handle(cmd, ctx).await,
        AtpDomain::Sys => sys::handle(cmd, ctx).await,
        AtpDomain::Pipe => pipe::handle(cmd, ctx).await,
        AtpDomain::Session => session::handle(cmd, ctx).await,
    }
}

/// Forward a command body directly to an IPC method and convert the result.
pub(super) async fn ipc_forward(
    id: &str,
    method: &str,
    params: Value,
    ipc: &dyn IpcRouter,
) -> AtpReply {
    match ipc.call(method, params).await {
        Ok(v) => AtpReply::ok(id, v),
        Err(e) => AtpReply::err(id, e),
    }
}

/// Produce an EPARSE unknown-op reply.
pub(super) fn unknown_op(id: impl Into<String>, op: &str) -> AtpReply {
    AtpReply::err(
        id,
        AtpError::new(AtpErrorCode::Eparse, format!("unknown op '{op}'")),
    )
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use crate::auth::atp_token::ATPTokenStore;
    use crate::auth::service::AuthService;
    use crate::config::auth::AuthPolicy;
    use crate::config::{AuthConfig, AuthIdentity, CredentialType};
    use crate::types::Role;
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    /// A mock IPC router for unit tests.
    /// Pre-load method → response pairs; anything not found returns Eunavail.
    pub struct MockIpcRouter {
        responses: Mutex<HashMap<String, Result<Value, AtpError>>>,
    }

    impl MockIpcRouter {
        pub fn new() -> Self {
            Self {
                responses: Mutex::new(HashMap::new()),
            }
        }

        pub async fn set_ok(&self, method: &str, value: Value) {
            self.responses
                .lock()
                .await
                .insert(method.to_string(), Ok(value));
        }

        #[allow(dead_code)]
        pub async fn set_err(&self, method: &str, err: AtpError) {
            self.responses
                .lock()
                .await
                .insert(method.to_string(), Err(err));
        }
    }

    #[async_trait]
    impl IpcRouter for MockIpcRouter {
        async fn call(&self, method: &str, _params: Value) -> Result<Value, AtpError> {
            self.responses
                .lock()
                .await
                .get(method)
                .cloned()
                .unwrap_or_else(|| {
                    Err(AtpError::new(
                        AtpErrorCode::Eunavail,
                        format!("mock: no response for '{method}'"),
                    ))
                })
        }
    }

    /// Build a minimal `HandlerCtx` with a mock IPC router pre-loaded with one response.
    pub async fn make_test_ctx(method: &str, response: Value) -> HandlerCtx {
        let mock = Arc::new(MockIpcRouter::new());
        mock.set_ok(method, response).await;
        HandlerCtx {
            ipc: mock,
            token_store: Arc::new(ATPTokenStore::new("test-secret".into())),
            hil_manager: None,
            auth_svc: Arc::new(AuthService::new(AuthConfig {
                api_version: "v1".into(),
                kind: "AuthConfig".into(),
                policy: AuthPolicy {
                    session_ttl: "8h".into(),
                    require_tls: false,
                },
                identities: vec![AuthIdentity {
                    name: "alice".into(),
                    uid: 1001,
                    role: Role::Admin,
                    credential: CredentialType::ApiKey {
                        key_hash: "key123".into(),
                        header: None,
                    },
                }],
            })),
        }
    }
}
