use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::auth::atp_token::ATPTokenStore;
use crate::auth::service::AuthService;
use crate::gateway::atp::error::{AtpError, AtpErrorCode};
use crate::gateway::atp::frame::AtpReply;
use crate::gateway::atp::types::AtpDomain;
use crate::gateway::validator::ValidatedCmd;
use crate::ipc::{message::JsonRpcRequest, IpcClient};

pub mod auth;
pub mod cap;
pub mod crews;
pub mod cron;
pub mod fs;
pub mod pipe;
pub mod proc;
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
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        };
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
